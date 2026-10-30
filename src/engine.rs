pub use crate::store::Settings;
pub use crate::AppState;
use crate::{
    models::*,
    store::{self, Config},
};
use anyhow::{bail, Result};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, RwLock, Semaphore};
#[derive(Clone, Default, Serialize)]
pub struct Status {
    pub status: String,
    pub latency_ms: Option<u64>,
    pub last_error: Option<String>,
}
#[derive(Clone, Serialize)]
pub struct Activity {
    pub timestamp: String,
    pub kind: String,
    pub message: String,
    pub indexer: Option<String>,
}
struct CacheEntry {
    at: Instant,
    results: Vec<Release>,
}
/// Process-unique runtime identities. Zero is never issued and exhaustion fails closed
/// instead of wrapping, so no epoch is ever reused within a process.
static NEXT_EPOCH: AtomicU64 = AtomicU64::new(1);
fn next_epoch(counter: &AtomicU64) -> Result<u64> {
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        let next = current
            .checked_add(1)
            .filter(|_| current != 0)
            .ok_or_else(|| {
                anyhow::anyhow!("Source runtime identities exhausted; restart Rackett")
            })?;
        match counter.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(epoch) => return Ok(epoch),
            Err(actual) => current = actual,
        }
    }
}
/// A download ticket whose producing runtime was replaced, deleted, or never existed.
#[derive(Debug)]
pub struct DownloadExpired;
impl std::fmt::Display for DownloadExpired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Download expired; search again")
    }
}
impl std::error::Error for DownloadExpired {}
/// What a ticketed download resolved to.
#[derive(Debug, PartialEq, Eq)]
pub enum Download {
    /// Bytes fetched from the ticket target itself, not yet validated as a torrent.
    Torrent {
        bytes: Vec<u8>,
        content_type: String,
    },
    /// A validated magnet URI selected from the ticket target's detail page.
    Magnet(String),
}
/// Configuration changes admitted at once: the one holding the writer plus those queued
/// behind it. Admission never waits, so abandoned requests cannot pile up detached work.
pub const MANAGEMENT_QUEUE: usize = 8;
/// A configuration change was refused before it started because too many are in flight.
#[derive(Debug)]
pub struct ManagementBusy;
impl std::fmt::Display for ManagementBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Too many configuration changes in progress; retry shortly")
    }
}
impl std::error::Error for ManagementBusy {}
const OUTCOME_UNKNOWN: &str =
    "Configuration change failed unexpectedly and its outcome is unknown; restart Rackett";
const RESTART_REQUIRED: &str =
    "Configuration changes are disabled after an internal error; restart Rackett";
struct Runtime {
    /// Configuration snapshot this runtime was compiled from; searches and downloads use
    /// it instead of re-reading config so source metadata and credentials never mix.
    indexer: Indexer,
    epoch: u64,
    builtin: Option<crate::builtin::Runtime>,
    /// Bundled sources only: saved cookie, solver and this epoch's clearance.
    source: Option<crate::builtin_runtime::Source>,
    generation: AtomicU64,
    client: reqwest::Client,
    gate: Mutex<Instant>,
    flight: Mutex<()>,
    cache: Mutex<HashMap<String, CacheEntry>>,
}
/// One source's answer to a search.
struct Hit {
    results: Vec<Release>,
    cached: bool,
    stale: bool,
}
/// Can this source carry anything in the requested categories? A parent category
/// (a round thousand) covers its children and a child is covered by its parent.
fn serves(i: &Indexer, cats: &[u32]) -> bool {
    let mine = crate::builtin::source_categories(i);
    cats.is_empty()
        || mine.is_empty()
        || cats.iter().any(|q| {
            mine.iter()
                .any(|c| q == c || (q / 1000 == c / 1000 && (q % 1000 == 0 || c % 1000 == 0)))
        })
}
pub struct Engine {
    path: PathBuf,
    sessions: crate::builtin_runtime::auth::SessionRepository,
    config: RwLock<Config>,
    /// Held by exactly one configuration change from merge through save and every
    /// in-memory effect; see [`Engine::transact`].
    write: Mutex<()>,
    admission: Arc<Semaphore>,
    /// Set when a change panicked: memory may no longer match disk, so refuse more.
    poisoned: AtomicBool,
    #[cfg(test)]
    test: TestHooks,
    runtime: RwLock<HashMap<String, Arc<Runtime>>>,
    pub statuses: RwLock<HashMap<String, Status>>,
    activity: Mutex<VecDeque<Activity>>,
    permits: Semaphore,
    searches: AtomicU64,
    hits: AtomicU64,
    failures: AtomicU64,
    started: Instant,
}
#[cfg(test)]
struct TestHooks {
    /// Fault injection consumed by this engine's next save only.
    save: std::sync::Mutex<Option<store::SaveHooks>>,
    /// Count of fully finished configuration changes, writer and permit released.
    finished: tokio::sync::watch::Sender<u64>,
}
#[cfg(test)]
struct Finished<'a>(&'a tokio::sync::watch::Sender<u64>);
#[cfg(test)]
impl Drop for Finished<'_> {
    fn drop(&mut self) {
        self.0.send_modify(|n| *n += 1);
    }
}
impl Engine {
    /// Load configuration and compile runtimes. Bootstrap is not a managed change: no
    /// other caller can observe the engine yet, and a first-start durability failure
    /// fails startup (see [`store::load`]) rather than serving an unconfirmed default.
    pub async fn new(path: PathBuf) -> Result<Self> {
        let config = store::load(&path).await?;
        if config.settings.port == 0
            || !(1..=120).contains(&config.settings.request_timeout_seconds)
            || config.settings.cache_ttl_seconds > 86400
            || config.settings.cache_stale_seconds > 604_800
            || config.settings.max_concurrent != 8
            || config.indexers.len() > 64
        {
            bail!("Invalid persisted settings; original configuration preserved")
        }
        if let Some(value) = &config.settings.public_url {
            store::normalize_public_url(value)?;
        }
        crate::builtin_runtime::flaresolverr::Solver::configured()?;
        let sessions = crate::builtin_runtime::auth::SessionRepository::new(&path);
        let mut runtime = HashMap::new();
        for i in &config.indexers {
            crate::adapters::validate(i)?;
            if i.id.is_empty()
                || runtime.contains_key(&i.id)
                || i.retries > 3
                || i.request_delay_ms > 60000
            {
                bail!("Invalid persisted indexer; original configuration preserved")
            }
            runtime.insert(i.id.clone(), Arc::new(Self::runtime(i, &sessions)?));
        }
        Ok(Self {
            path,
            sessions,
            config: RwLock::new(config),
            write: Mutex::new(()),
            admission: Arc::new(Semaphore::new(MANAGEMENT_QUEUE)),
            poisoned: AtomicBool::new(false),
            #[cfg(test)]
            test: TestHooks {
                save: std::sync::Mutex::new(None),
                finished: tokio::sync::watch::Sender::new(0),
            },
            runtime: RwLock::new(runtime),
            statuses: RwLock::new(HashMap::new()),
            activity: Mutex::new(VecDeque::new()),
            permits: Semaphore::new(8),
            searches: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            failures: AtomicU64::new(0),
            started: Instant::now(),
        })
    }
    fn runtime(
        i: &Indexer,
        sessions: &crate::builtin_runtime::auth::SessionRepository,
    ) -> Result<Runtime> {
        let builtin = if i.kind == "cardigann" {
            Some(crate::builtin::Runtime::compile(i)?)
        } else {
            None
        };
        let mut runtime = Self::runtime_with(i, builtin)?;
        if let Some(session) = runtime
            .source
            .as_mut()
            .and_then(|s| s.cookie_session.as_mut())
        {
            session.bind(sessions, &i.id, crate::builtin::id(i)?)?;
        }
        Ok(runtime)
    }
    fn runtime_with(i: &Indexer, builtin: Option<crate::builtin::Runtime>) -> Result<Runtime> {
        let solver = if builtin.is_some() {
            crate::builtin_runtime::flaresolverr::Solver::configured()?
        } else {
            None
        };
        let mut headers = reqwest::header::HeaderMap::new();
        // A bundled source composes its saved cookie per request with the plan's own.
        if !i.cookie.is_empty() && builtin.is_none() {
            headers.insert(reqwest::header::COOKIE, i.cookie.parse()?);
        }
        let origin = url::Url::parse(&i.url)?.origin();
        let stop = i.kind == "cardigann";
        let client = reqwest::Client::builder()
            .retry(reqwest::retry::never())
            .default_headers(headers)
            .user_agent("Rackett/0.1")
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if stop {
                    attempt.stop()
                } else if attempt.previous().len() >= 5 {
                    attempt.error("Too many redirects")
                } else if attempt.url().origin() != origin
                    || !matches!(attempt.url().scheme(), "http" | "https")
                    || !attempt.url().username().is_empty()
                    || attempt.url().password().is_some()
                {
                    attempt.error("Cross-origin redirect rejected")
                } else {
                    attempt.follow()
                }
            }))
            .connect_timeout(Duration::from_secs(10));
        // Definition `certificates` pin trust for this source's site host only.
        let pins = if builtin.is_some() {
            crate::builtin::certificates(i)?
        } else {
            None
        };
        let client = crate::pinned_tls::apply(client, pins.as_ref(), &i.url)?;
        // Clearance is tied to the solver's egress. Only explicitly admitted
        // solver-backed sources use the operator's optional address-family pin.
        let client = if crate::builtin::id(i)
            .is_ok_and(|id| builtin.is_some() && crate::builtin::CHALLENGE_IDS.contains(&id))
        {
            if let Some(solver) = &solver {
                solver.source_client(client)
            } else {
                client
            }
        } else {
            client
        }
        .build()?;
        Ok(Runtime {
            indexer: i.clone(),
            epoch: next_epoch(&NEXT_EPOCH)?,
            source: if builtin.is_some() {
                Some(crate::builtin_runtime::Source::new(i, solver)?)
            } else {
                None
            },
            builtin,
            generation: AtomicU64::new(0),
            client,
            gate: Mutex::new(Instant::now()),
            flight: Mutex::new(()),
            cache: Mutex::new(HashMap::new()),
        })
    }
    pub async fn indexers(&self) -> Vec<Indexer> {
        self.config.read().await.indexers.clone()
    }
    pub async fn indexer(&self, id: &str) -> Option<Indexer> {
        self.indexers().await.into_iter().find(|i| i.id == id)
    }
    /// Run one configuration change as an owned task holding the writer lock throughout
    /// merge, validation, save and every in-memory effect.
    ///
    /// Admission happens on the caller's first poll; a future never polled changes
    /// nothing. Once admitted, the change completes even if the caller's future is
    /// dropped (an HTTP client disconnecting, say), so a late save can never land after
    /// the writer was released and memory never stops halfway behind disk. At most
    /// [`MANAGEMENT_QUEUE`] changes are admitted; more fail at once with [`ManagementBusy`].
    ///
    /// A panicking change releases the writer, reports an unknown outcome and disables
    /// further changes until restart, since memory may no longer match disk. Runtime
    /// shutdown can still stop an admitted change at an await point; the file is then the
    /// previous or the new complete configuration and the next start loads it.
    async fn transact<T, F, Fut>(self: &Arc<Self>, change: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(Arc<Self>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T>> + Send + 'static,
    {
        use futures_util::FutureExt;
        if self.poisoned.load(Ordering::SeqCst) {
            bail!(RESTART_REQUIRED)
        }
        let permit = self
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| ManagementBusy)?;
        let engine = self.clone();
        let task = tokio::spawn(async move {
            // Counted even if this task is dropped, so tests fail rather than hang.
            #[cfg(test)]
            let _finished = Finished(&engine.test.finished);
            let permit = permit;
            let writer = engine.write.lock().await;
            let result = if engine.poisoned.load(Ordering::SeqCst) {
                Err(anyhow::anyhow!(RESTART_REQUIRED))
            } else {
                std::panic::AssertUnwindSafe(change(engine.clone()))
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| {
                        engine.poisoned.store(true, Ordering::SeqCst);
                        Err(anyhow::anyhow!(OUTCOME_UNKNOWN))
                    })
            };
            // Return the permit first so whoever takes the writer next sees it free.
            drop(permit);
            drop(writer);
            result
        });
        task.await.unwrap_or_else(|_| {
            self.poisoned.store(true, Ordering::SeqCst);
            Err(anyhow::anyhow!(OUTCOME_UNKNOWN))
        })
    }
    /// Save `c`; only call inside [`Engine::transact`]. An error means nothing became
    /// visible. [`store::Commit::Unconfirmed`] means the file was replaced: apply every
    /// in-memory effect, then report it with [`store::Commit::confirm`].
    async fn commit(&self, c: &Config) -> Result<store::Commit> {
        #[cfg(test)]
        let hooks = self.test.save.lock().unwrap().take().unwrap_or_default();
        #[cfg(not(test))]
        let hooks = store::SaveHooks::default();
        store::commit(&self.path, c, hooks).await
    }
    pub async fn set_port(self: &Arc<Self>, port: u16) -> Result<()> {
        self.transact(move |e| async move { e.set_port_locked(port).await })
            .await
    }
    async fn set_port_locked(&self, port: u16) -> Result<()> {
        let mut c = self.config.read().await.clone();
        c.settings.port = port;
        let saved = self.commit(&c).await?;
        *self.config.write().await = c;
        saved.confirm()
    }
    pub async fn set_public_url(self: &Arc<Self>, value: &str) -> Result<()> {
        let url = store::normalize_public_url(value)?;
        self.transact(move |e| async move { e.set_public_url_locked(url).await })
            .await
    }
    async fn set_public_url_locked(&self, url: String) -> Result<()> {
        let mut c = self.config.read().await.clone();
        if c.settings.public_url.as_deref() == Some(&url) {
            return Ok(());
        }
        c.settings.public_url = Some(url);
        let saved = self.commit(&c).await?;
        *self.config.write().await = c;
        saved.confirm()
    }
    pub async fn settings(&self) -> Settings {
        self.config.read().await.settings.clone()
    }
    pub async fn update_settings(self: &Arc<Self>, patch: serde_json::Value) -> Result<Settings> {
        self.transact(move |e| async move { e.update_settings_locked(patch).await })
            .await
    }
    async fn update_settings_locked(&self, patch: serde_json::Value) -> Result<Settings> {
        let mut c = self.config.read().await.clone();
        for (k, v) in patch
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Expected settings object"))?
        {
            match k.as_str() {
                "cache_ttl_seconds" => {
                    c.settings.cache_ttl_seconds = v
                        .as_u64()
                        .filter(|n| *n <= 86400)
                        .ok_or_else(|| anyhow::anyhow!("TTL must be 0..86400"))?
                }
                "request_timeout_seconds" => {
                    c.settings.request_timeout_seconds = v
                        .as_u64()
                        .filter(|n| (1..=120).contains(n))
                        .ok_or_else(|| anyhow::anyhow!("Timeout must be 1..120"))?
                }
                "cache_stale_seconds" => {
                    c.settings.cache_stale_seconds = v
                        .as_u64()
                        .filter(|n| *n <= 604_800)
                        .ok_or_else(|| anyhow::anyhow!("Stale window must be 0..604800"))?
                }
                _ => bail!("Unsupported setting: {k}"),
            }
        }
        let saved = self.commit(&c).await?;
        let s = c.settings.clone();
        *self.config.write().await = c;
        self.clear_cache().await;
        saved.confirm()?;
        Ok(s)
    }
    /// A rotation whose save is visible but unconfirmed still replaces the live key; the
    /// caller then gets [`store::DurabilityUnconfirmed`] and must re-read the settings.
    pub async fn rotate_key(self: &Arc<Self>) -> Result<Settings> {
        self.transact(|e| async move { e.rotate_key_locked().await })
            .await
    }
    async fn rotate_key_locked(&self) -> Result<Settings> {
        let mut c = self.config.read().await.clone();
        c.settings.api_key = store::random_key();
        let saved = self.commit(&c).await?;
        let s = c.settings.clone();
        *self.config.write().await = c;
        saved.confirm()?;
        Ok(s)
    }
    /// Merge against the current source while holding the same lock used to save it.
    /// A queued PATCH must not restore credentials or a URL read before another edit.
    pub async fn patch_indexer(
        self: &Arc<Self>,
        id: &str,
        patch: serde_json::Value,
    ) -> Result<Indexer> {
        let id = id.to_owned();
        self.transact(move |e| async move { e.patch_indexer_locked(&id, patch).await })
            .await
    }
    async fn patch_indexer_locked(&self, id: &str, patch: serde_json::Value) -> Result<Indexer> {
        let old = self
            .indexer(id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Indexer not found"))?;
        let patch = patch
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Expected object"))?;
        let target = patch
            .get("url")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&old.url);
        let public = crate::builtin::public_definition(&old)
            .filter(|_| !crate::builtin::same_origin(&old.url, target));
        let mut merged = serde_json::to_value(&old)?;
        if let Some(definition) = public {
            merged["definition"] = definition;
            // A bundled source's saved cookie stays with its origin, like preset secrets.
            merged["cookie"] = serde_json::json!("");
        }
        for (key, value) in patch {
            if key != "id" {
                merged[key] = value.clone();
            }
        }
        self.put_indexer_locked(serde_json::from_value(merged)?, true)
            .await
    }
    /// A created source whose save is visible but unconfirmed is live under its new ID;
    /// the caller then gets [`store::DurabilityUnconfirmed`] without that ID.
    pub async fn put_indexer(self: &Arc<Self>, i: Indexer, existing: bool) -> Result<Indexer> {
        self.transact(move |e| async move { e.put_indexer_locked(i, existing).await })
            .await
    }
    async fn put_indexer_locked(&self, mut i: Indexer, existing: bool) -> Result<Indexer> {
        if i.name.trim().is_empty() || i.name.len() > 200 {
            bail!("Name must be 1..200 characters")
        }
        if i.retries > 3 || i.request_delay_ms > 60000 {
            bail!("Retries maximum 3; delay maximum 60000ms")
        }
        if existing {
            if let Some(old) = self.indexer(&i.id).await {
                crate::builtin::preserve_secrets(&mut i, &old);
            }
        }
        crate::adapters::validate(&i)?;
        if !existing {
            i.id = format!("custom-{}", &store::random_key()[..12]);
        }
        let r = Arc::new(Self::runtime(&i, &self.sessions)?);
        let mut c = self.config.read().await.clone();
        if existing {
            let slot = c
                .indexers
                .iter_mut()
                .find(|x| x.id == i.id)
                .ok_or_else(|| anyhow::anyhow!("Indexer not found"))?;
            *slot = i.clone();
        } else {
            if c.indexers.len() >= 64 {
                bail!("Maximum 64 configured sources")
            }
            c.indexers.push(i.clone());
        }
        let saved = self.commit(&c).await?;
        self.runtime.write().await.insert(i.id.clone(), r);
        *self.config.write().await = c;
        self.statuses.write().await.remove(&i.id);
        self.log(
            "configuration",
            format!("Saved {}", i.name),
            Some(i.id.clone()),
        )
        .await;
        saved.confirm()?;
        Ok(i)
    }
    pub async fn delete_indexer(self: &Arc<Self>, id: &str) -> Result<()> {
        let id = id.to_owned();
        self.transact(move |e| async move { e.delete_indexer_locked(&id).await })
            .await
    }
    async fn delete_indexer_locked(&self, id: &str) -> Result<()> {
        let mut c = self.config.read().await.clone();
        if !c.indexers.iter().any(|i| i.id == id) {
            bail!("Indexer not found")
        }
        c.indexers.retain(|i| i.id != id);
        let saved = self.commit(&c).await?;
        self.runtime.write().await.remove(id);
        *self.config.write().await = c;
        self.statuses.write().await.remove(id);
        saved.confirm()
    }
    pub async fn clear_cache(&self) {
        for r in self.runtime.read().await.values() {
            r.generation.fetch_add(1, Ordering::Relaxed);
            r.cache.lock().await.clear();
        }
    }
    pub async fn cache_entries(&self) -> usize {
        let runtimes: Vec<_> = self.runtime.read().await.values().cloned().collect();
        let mut entries = 0;
        for r in runtimes {
            entries += r.cache.lock().await.len();
        }
        entries
    }
    pub async fn activity(&self) -> Vec<Activity> {
        self.activity.lock().await.iter().cloned().collect()
    }
    async fn log(&self, kind: &str, message: String, indexer: Option<String>) {
        let mut a = self.activity.lock().await;
        a.push_front(Activity {
            timestamp: chrono::Utc::now().to_rfc3339(),
            kind: kind.into(),
            message,
            indexer,
        });
        a.truncate(100);
    }
    pub fn stats(&self) -> serde_json::Value {
        serde_json::json!({"searches":self.searches.load(Ordering::Relaxed),"cache_hits":self.hits.load(Ordering::Relaxed),"errors":self.failures.load(Ordering::Relaxed),"uptime_seconds":self.started.elapsed().as_secs()})
    }
    /// Search the selected sources. `maxwait` (seconds) bounds the whole aggregate: sources
    /// that have not answered by then are reported as pending and keep searching so their
    /// results are cached for the next call. Without it, every source is awaited.
    pub async fn search(self: &Arc<Self>, ids: Vec<String>, query: SearchQuery) -> SearchResponse {
        let aggregate = ids.is_empty() || ids.iter().any(|id| id == "all");
        self.search_selected(ids, query, aggregate).await
    }
    /// Preserve aggregate semantics when a protocol has already narrowed the source IDs
    /// by their query capabilities. Explicit source requests still bypass category skipping.
    pub(crate) async fn search_selected(
        self: &Arc<Self>,
        ids: Vec<String>,
        query: SearchQuery,
        aggregate: bool,
    ) -> SearchResponse {
        let started = Instant::now();
        self.searches.fetch_add(1, Ordering::Relaxed);
        let indexers = self.indexers().await;
        let cats: Vec<u32> = query
            .get("cat")
            .split(',')
            .filter_map(|s| s.parse().ok())
            .collect();
        let mut out = SearchResponse::default();
        let selected: Vec<_> = indexers
            .into_iter()
            .filter(|i| {
                i.enabled && (ids.is_empty() || ids.iter().any(|id| id == "all" || id == &i.id))
            })
            .collect();
        for id in &ids {
            if id != "all" && !selected.iter().any(|i| &i.id == id) {
                out.errors.push(SearchError {
                    indexer_id: id.clone(),
                    indexer: id.clone(),
                    message: "Unknown or disabled indexer".into(),
                });
            }
        }
        // An aggregate never asks a source for a category it does not publish: that only
        // costs time, and the slowest such member used to bound the whole answer. A source
        // named explicitly is always asked, custom categories included.
        let selected: Vec<_> = selected
            .into_iter()
            .filter(|i| {
                let asked = !aggregate || serves(i, &cats);
                if !asked {
                    out.skipped.push(i.name.clone());
                }
                asked
            })
            .collect();
        let mut adapter_query = query.clone();
        adapter_query.params.remove("offset");
        adapter_query.params.remove("maxwait");
        let maxwait = query
            .get("maxwait")
            .parse::<f64>()
            .ok()
            .filter(|s| s.is_finite() && *s >= 0.0)
            .map(|s| Duration::from_secs_f64(s.min(120.0)));
        let fetch = query
            .get("offset")
            .parse::<usize>()
            .unwrap_or(0)
            .saturating_add(query.get("limit").parse::<usize>().unwrap_or(1000))
            .clamp(1, 1000);
        adapter_query
            .params
            .insert("limit".into(), fetch.to_string());
        // Each source runs as its own task so that giving up on it (maxwait) or losing the
        // client does not abandon the fetch: it completes and fills the cache.
        let jobs: Vec<_> = selected
            .iter()
            .map(|i| {
                let (engine, i, q) = (self.clone(), i.clone(), adapter_query.clone());
                tokio::spawn(async move { engine.one(&i, &q, false).await })
            })
            .collect();
        for (i, job) in selected.iter().zip(jobs) {
            let joined = match maxwait {
                Some(wait) => {
                    match tokio::time::timeout(wait.saturating_sub(started.elapsed()), job).await {
                        Ok(joined) => joined,
                        Err(_) => {
                            out.pending.push(SearchPending {
                                indexer_id: i.id.clone(),
                                indexer: i.name.clone(),
                            });
                            continue;
                        }
                    }
                }
                None => job.await,
            };
            match joined {
                Ok(Ok(hit)) => {
                    out.cached += usize::from(hit.cached);
                    out.stale += usize::from(hit.stale);
                    let mut results = hit.results;
                    out.results.append(&mut results)
                }
                Ok(Err(e)) => out.errors.push(SearchError {
                    indexer_id: i.id.clone(),
                    indexer: i.name.clone(),
                    message: e.to_string(),
                }),
                Err(_) => out.errors.push(SearchError {
                    indexer_id: i.id.clone(),
                    indexer: i.name.clone(),
                    message: "Search task failed unexpectedly".into(),
                }),
            }
        }
        let mut seen = HashSet::new();
        out.results
            .retain(|r| seen.insert((r.indexer_id.clone(), r.guid.clone())));
        out.results.sort_by(|a, b| b.published.cmp(&a.published));
        let offset = query.get("offset").parse::<usize>().unwrap_or(0);
        let limit = query
            .get("limit")
            .parse::<usize>()
            .unwrap_or(1000)
            .min(1000);
        out.results = out.results.into_iter().skip(offset).take(limit).collect();
        out.elapsed_ms = started.elapsed().as_millis() as u64;
        self.log(
            "search",
            format!(
                "{} results · {} errors · {} pending · {} stale · {} ms",
                out.results.len(),
                out.errors.len(),
                out.pending.len(),
                out.stale,
                out.elapsed_ms
            ),
            None,
        )
        .await;
        out
    }
    pub async fn test(self: &Arc<Self>, id: &str) -> Result<u64> {
        let i = self
            .indexer(id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Indexer not found"))?;
        let start = Instant::now();
        self.one(&i, &SearchQuery::default(), true).await?;
        Ok(start.elapsed().as_millis() as u64)
    }
    async fn one(self: &Arc<Self>, i: &Indexer, q: &SearchQuery, test: bool) -> Result<Hit> {
        let r = self
            .runtime
            .read()
            .await
            .get(&i.id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Indexer changed"))?;
        if !test && !r.indexer.enabled {
            bail!("Indexer changed");
        }
        let settings = self.settings().await;
        let key = serde_json::to_string(q)?;
        let bypass = test || q.get("cache") == "false";
        match self.lookup(&r, &key, bypass, &settings).await {
            Some((results, false)) => Ok(Hit {
                results,
                cached: true,
                stale: false,
            }),
            Some((results, true)) => {
                // Best answer right now: the expired set, while one refresh runs behind the
                // flight lock. A refresh already in flight is simply joined by the next miss.
                if r.flight.try_lock().is_ok() {
                    let (engine, r, q) = (self.clone(), r.clone(), q.clone());
                    tokio::spawn(async move {
                        let _ = engine.fetch(&r, &q, false).await;
                    });
                }
                Ok(Hit {
                    results,
                    cached: true,
                    stale: true,
                })
            }
            None => self
                .fetch(&r, q, bypass)
                .await
                .map(|(results, cached)| Hit {
                    results,
                    cached,
                    stale: false,
                }),
        }
    }
    /// A cached result set for `key`: `(results, stale)` where stale means older than the
    /// TTL but inside the stale window. Entries past both are dropped.
    async fn lookup(
        &self,
        r: &Runtime,
        key: &str,
        bypass: bool,
        settings: &Settings,
    ) -> Option<(Vec<Release>, bool)> {
        if bypass {
            return None;
        }
        let ttl = Duration::from_secs(settings.cache_ttl_seconds);
        let keep = ttl.max(Duration::from_secs(settings.cache_stale_seconds));
        let mut cache = r.cache.lock().await;
        cache.retain(|_, e| e.at.elapsed() < keep);
        let entry = cache.get(key)?;
        self.hits.fetch_add(1, Ordering::Relaxed);
        Some((entry.results.clone(), entry.at.elapsed() >= ttl))
    }
    /// Fetch from the source behind the flight lock and record its status. With `bypass`
    /// nothing is read from or written to the cache. Returns the results and whether
    /// another fetch had already cached them meanwhile.
    async fn fetch(
        &self,
        r: &Arc<Runtime>,
        q: &SearchQuery,
        bypass: bool,
    ) -> Result<(Vec<Release>, bool)> {
        let r = r.clone();
        let i = &r.indexer;
        let settings = self.settings().await;
        let generation = r.generation.load(Ordering::Relaxed);
        let start = Instant::now();
        let deadline = start + Duration::from_secs(settings.request_timeout_seconds);
        let key = serde_json::to_string(q)?;
        let task = async {
            // A warm query must not wait behind an unrelated slow fetch. Recheck
            // after admission so simultaneous misses still share one request.
            if let Some((results, false)) = self.lookup(&r, &key, bypass, &settings).await {
                return Ok((results, true));
            }
            let _flight = r.flight.lock().await;
            if let Some((results, false)) = self.lookup(&r, &key, bypass, &settings).await {
                return Ok((results, true));
            }
            let _permit = self.permits.acquire().await?;
            let mut gate = r.gate.lock().await;
            let mut attempts = 0;
            let releases = loop {
                if r.builtin.is_none() {
                    tokio::time::sleep_until((*gate).into()).await;
                    *gate = Instant::now()
                        + Duration::from_millis(
                            r.builtin
                                .as_ref()
                                .map(|b| b.delay_ms)
                                .unwrap_or(i.request_delay_ms),
                        );
                }
                let result = if let (Some(builtin), Some(source)) = (&r.builtin, &r.source) {
                    crate::builtin_runtime::search_source(
                        builtin, &r.client, i, q, &mut gate, source, deadline,
                    )
                    .await
                } else {
                    crate::adapters::search(&r.client, i, q).await
                };
                match result {
                    Ok(v) => break v,
                    Err(e) => {
                        let transient = e
                            .chain()
                            .find_map(|x| x.downcast_ref::<reqwest::Error>())
                            .map(|e| {
                                e.is_timeout()
                                    || e.is_connect()
                                    || e.status().is_some_and(|s| s.is_server_error())
                            })
                            .unwrap_or(false);
                        if r.builtin.is_some() || !transient || attempts >= i.retries {
                            *gate = Instant::now()
                                + Duration::from_millis(
                                    r.builtin
                                        .as_ref()
                                        .map(|b| b.delay_ms)
                                        .unwrap_or(i.request_delay_ms),
                                );
                            return Err(e);
                        }
                        attempts += 1;
                        tokio::time::sleep(Duration::from_millis(250 * (1u64 << attempts))).await;
                    }
                }
            };
            *gate = Instant::now()
                + Duration::from_millis(
                    r.builtin
                        .as_ref()
                        .map(|b| b.delay_ms)
                        .unwrap_or(i.request_delay_ms),
                );
            drop(gate);
            let cats: Vec<u32> = q
                .get("cat")
                .split(',')
                .filter_map(|s| s.parse().ok())
                .collect();
            let mut releases: Vec<_> = releases
                .into_iter()
                .map(|mut release| {
                    // Stamp before caching or returning so every copy keeps its producer.
                    release.source_epoch = r.epoch;
                    release
                })
                .filter(|r| {
                    cats.is_empty()
                        || r.categories.is_empty()
                        || r.categories.iter().any(|c| {
                            cats.iter()
                                .any(|q| q == c || (*q % 1000 == 0 && c / 1000 == q / 1000))
                        })
                })
                .take(1000)
                .collect();
            // `search` asks each source for offset + limit candidates. Deduplicate
            // before ranking, just as the aggregate does, then retain only that
            // prefix. No later candidate from this source can enter the global
            // prefix; caching entire multi-page responses only adds clone/sort work.
            let mut seen = HashSet::new();
            releases.retain(|r| seen.insert(r.guid.clone()));
            releases.sort_by(|a, b| b.published.cmp(&a.published));
            releases.truncate(
                q.get("limit")
                    .parse::<usize>()
                    .unwrap_or(1000)
                    .clamp(1, 1000),
            );
            if !bypass && settings.cache_ttl_seconds > 0 {
                let mut c = r.cache.lock().await;
                if r.generation.load(Ordering::Relaxed) != generation {
                    return Ok((releases, false));
                }
                if c.len() >= 64 {
                    if let Some(old) = c.iter().min_by_key(|(_, e)| e.at).map(|(k, _)| k.clone()) {
                        c.remove(&old);
                    }
                }
                c.insert(
                    key.clone(),
                    CacheEntry {
                        at: Instant::now(),
                        results: releases.clone(),
                    },
                );
            }
            Ok((releases, false))
        };
        let result =
            tokio::time::timeout(Duration::from_secs(settings.request_timeout_seconds), task)
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Indexer deadline exceeded")));
        let ms = start.elapsed().as_millis() as u64;
        let status = match &result {
            Ok(_) => Status {
                status: "healthy".into(),
                latency_ms: Some(ms),
                last_error: None,
            },
            Err(e) => {
                self.failures.fetch_add(1, Ordering::Relaxed);
                Status {
                    status: "error".into(),
                    latency_ms: Some(ms),
                    last_error: Some(if r.builtin.is_some() {
                        e.to_string()
                    } else {
                        "Request failed; check feed URL, credentials, or timeout".into()
                    }),
                }
            }
        };
        if self
            .runtime
            .read()
            .await
            .get(&i.id)
            .is_some_and(|current| Arc::ptr_eq(current, &r))
        {
            self.statuses.write().await.insert(i.id.clone(), status);
        }
        result.map_err(|e| {
            if r.builtin.is_some() {
                e
            } else {
                anyhow::anyhow!("Request failed; check URL, credentials, format, or timeout")
            }
        })
    }
    /// Fetch a ticketed download only through the exact runtime that produced its release.
    /// Any edit, delete, or engine recreation retires the runtime and its tickets. A
    /// runtime with a detail-page download policy resolves the target page to a magnet.
    pub async fn download(&self, id: &str, epoch: u64, target: &str) -> Result<Download> {
        let r = self
            .runtime
            .read()
            .await
            .get(id)
            .filter(|r| epoch != 0 && r.epoch == epoch && r.indexer.enabled)
            .cloned()
            .ok_or(DownloadExpired)?;
        let i = &r.indexer;
        let url = url::Url::parse(target)?;
        let base = url::Url::parse(&i.url)?;
        if !matches!(url.scheme(), "http" | "https")
            || url.origin() != base.origin()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            bail!("Only same-origin HTTP downloads are supported")
        }
        let detail = r.builtin.as_ref().and_then(|b| b.download);
        if detail.is_some() {
            crate::builtin_runtime::detail_target(&base, target)?;
        }
        // General `download.selectors` resolution, compiled from the pinned definition.
        let selectors = match r.builtin {
            Some(_) => crate::builtin::download_selectors(i)?.map(Arc::new),
            None => None,
        };
        let seconds = self.settings().await.request_timeout_seconds;
        let deadline = Instant::now() + Duration::from_secs(seconds);
        tokio::time::timeout(Duration::from_secs(seconds), async {
            let _permit = self.permits.acquire().await?;
            let mut gate = r.gate.lock().await;
            tokio::time::sleep_until((*gate).into()).await;
            // Revalidate after queueing: a source edited while this waited sends nothing.
            // A request that passed this check is in flight and completes even if an edit
            // lands meanwhile; it uses the credentials current at this admission check.
            if !self
                .runtime
                .read()
                .await
                .get(id)
                .is_some_and(|current| Arc::ptr_eq(current, &r))
            {
                return Err(DownloadExpired.into());
            }
            if let Some(policy) = detail {
                let delay_ms = r.builtin.as_ref().map_or(0, |b| b.delay_ms);
                let source = r
                    .source
                    .as_ref()
                    .ok_or(crate::builtin_runtime::DetailFailure::Fetch)?;
                let magnet = crate::builtin_runtime::detail_magnet_source(
                    policy,
                    &mut crate::builtin_runtime::Transport {
                        client: &r.client,
                        source,
                        base: &base,
                        gate: &mut gate,
                        delay_ms,
                        deadline,
                    },
                    &url,
                )
                .await;
                *gate = Instant::now() + Duration::from_millis(delay_ms);
                return magnet.map(Download::Magnet);
            }
            if let (Some(selectors), Some(b)) = (selectors.clone(), r.builtin.as_ref()) {
                let delay_ms = b.delay_ms;
                let source = r
                    .source
                    .as_ref()
                    .ok_or(crate::builtin_runtime::DetailFailure::Fetch)?;
                let resolved = crate::builtin_runtime::resolve_download_selectors(
                    selectors,
                    b,
                    &mut crate::builtin_runtime::Transport {
                        client: &r.client,
                        source,
                        base: &base,
                        gate: &mut gate,
                        delay_ms,
                        deadline,
                    },
                    &url,
                )
                .await;
                *gate = Instant::now() + Duration::from_millis(delay_ms);
                // `None`: a before-only block; the link itself is downloaded below.
                if let Some(download) = resolved? {
                    return Ok(download);
                }
            }
            if let Some(session) = r.source.as_ref().and_then(|s| s.cookie_session.as_ref()) {
                return Ok(Download::Torrent {
                    bytes: session.fetch_download(target).await?,
                    content_type: "application/x-bittorrent".into(),
                });
            }
            if let Some(session) = r.source.as_ref().and_then(|s| s.api_session.as_ref()) {
                return Ok(Download::Torrent {
                    bytes: session
                        .fetch_download(
                            &r.client,
                            target,
                            &mut gate,
                            r.builtin.as_ref().map_or(0, |b| b.delay_ms),
                        )
                        .await?,
                    content_type: "application/x-bittorrent".into(),
                });
            }
            *gate = Instant::now()
                + Duration::from_millis(
                    r.builtin
                        .as_ref()
                        .map(|b| b.delay_ms)
                        .unwrap_or(i.request_delay_ms),
                );
            let mut request = r.client.get(url);
            if let (Some(b), Some(source)) = (r.builtin.as_ref(), r.source.as_ref()) {
                request = request.headers(crate::builtin_runtime::get_login_download_headers(
                    b, source,
                )?);
            }
            if i.kind == "unit3d" && !i.api_key.is_empty() {
                request = request.bearer_auth(&i.api_key);
            }
            let response = request.send().await?.error_for_status()?;
            if !response.status().is_success() {
                bail!("Download did not return a successful response");
            }
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|h| h.to_str().ok())
                .unwrap_or("application/x-bittorrent")
                .to_owned();
            if response
                .content_length()
                .is_some_and(|n| n > 32 * 1024 * 1024)
            {
                bail!("Download too large")
            }
            let mut stream = response.bytes_stream();
            use futures_util::StreamExt;
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                if bytes.len() + chunk.len() > 32 * 1024 * 1024 {
                    bail!("Download too large")
                }
                bytes.extend_from_slice(&chunk);
            }
            *gate = Instant::now()
                + Duration::from_millis(
                    r.builtin
                        .as_ref()
                        .map(|b| b.delay_ms)
                        .unwrap_or(i.request_delay_ms),
                );
            Ok(Download::Torrent {
                bytes,
                content_type,
            })
        })
        .await
        .map_err(|_| anyhow::anyhow!("Download deadline exceeded"))?
    }
}

#[cfg(test)]
mod source_update_tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use serde_json::json;
    use tower::ServiceExt;

    #[tokio::test]
    async fn queued_api_patches_merge_current_origin_and_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let engine = Arc::new(Engine::new(path.clone()).await.unwrap());
        let source = engine
            .put_indexer(
                Indexer {
                    name: "Original".into(),
                    kind: "cardigann".into(),
                    url: "http://127.0.0.1:9/".into(),
                    definition: Some(
                        json!({"builtin_id":"milkie","settings":{"apikey":"old-origin-key"}}),
                    ),
                    ..Default::default()
                },
                false,
            )
            .await
            .unwrap();
        let app = crate::api::router().with_state(AppState {
            engine: engine.clone(),
        });
        let request = |patch: serde_json::Value| {
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/indexers/{}", source.id))
                .header("host", "localhost")
                .header("origin", "http://localhost")
                .header("content-type", "application/json")
                .body(Body::from(patch.to_string()))
                .unwrap()
        };
        // Hold the actual writer lock, then queue two real API requests in order.
        // The second must read the state saved by the first after it gets the lock.
        let writer = engine.write.lock().await;
        let mut first = Box::pin(app.clone().oneshot(request(json!({
            "url":"http://127.0.0.1:10/",
            "definition":{"builtin_id":"milkie","settings":{"apikey":"new-origin-key"}}
        }))));
        assert!(futures_util::poll!(&mut first).is_pending());
        let mut second = Box::pin(app.oneshot(request(json!({"name":"Renamed"}))));
        assert!(futures_util::poll!(&mut second).is_pending());
        drop(writer);
        let (first, second) = tokio::join!(first, second);
        assert!(first.unwrap().status().is_success());
        assert!(second.unwrap().status().is_success());
        let saved = engine.indexer(&source.id).await.unwrap();
        assert_eq!(saved.name, "Renamed");
        assert_eq!(saved.url, "http://127.0.0.1:10/");
        assert_eq!(
            saved.definition.as_ref().unwrap()["settings"]["apikey"],
            "new-origin-key"
        );
        let restarted = Engine::new(path).await.unwrap();
        assert_eq!(
            restarted.indexer(&source.id).await.unwrap().definition,
            saved.definition
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn stale_selection_cannot_search_or_download_through_disabled_runtime() {
        use axum::{routing::get, Router};
        let requests = Arc::new(AtomicU64::new(0));
        let seen = requests.clone();
        let app = Router::new().fallback(get(move || {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::Relaxed);
                "<rss version=\"2.0\"><channel><title>Fixture</title></channel></rss>"
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/feed", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let engine = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
        let selected = engine
            .put_indexer(
                Indexer {
                    name: "Fixture".into(),
                    url: url.clone(),
                    request_delay_ms: 0,
                    ..Default::default()
                },
                false,
            )
            .await
            .unwrap();
        let mut disabled = selected.clone();
        disabled.enabled = false;
        engine.put_indexer(disabled, true).await.unwrap();
        // Emulate search() selecting the source before its disabled runtime is published.
        assert!(engine
            .one(&selected, &SearchQuery::default(), false)
            .await
            .is_err());
        assert_eq!(requests.load(Ordering::Relaxed), 0);
        // An explicit management test remains permitted, but its disabled runtime cannot download.
        engine.test(&selected.id).await.unwrap();
        assert_eq!(requests.load(Ordering::Relaxed), 1);
        let epoch = engine.runtime.read().await[&selected.id].epoch;
        assert!(engine
            .download(&selected.id, epoch, &url)
            .await
            .unwrap_err()
            .is::<DownloadExpired>());
        assert_eq!(requests.load(Ordering::Relaxed), 1);
        server.abort();
    }

    #[test]
    fn epochs_are_nonzero_and_never_wrap() {
        let counter = AtomicU64::new(u64::MAX - 1);
        assert_eq!(next_epoch(&counter).unwrap(), u64::MAX - 1);
        assert!(next_epoch(&counter).is_err());
        assert!(next_epoch(&counter).is_err());
        assert!(next_epoch(&AtomicU64::new(0)).is_err());
        let fresh = AtomicU64::new(1);
        assert_eq!(next_epoch(&fresh).unwrap(), 1);
        assert_eq!(next_epoch(&fresh).unwrap(), 2);
    }

    #[test]
    fn concurrent_epoch_allocations_are_unique() {
        let counter = AtomicU64::new(1);
        let barrier = std::sync::Barrier::new(8);
        let mut epochs = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        (0..1000)
                            .map(|_| next_epoch(&counter).unwrap())
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        });
        epochs.sort_unstable();
        assert_eq!(epochs, (1..=8000).collect::<Vec<_>>());
        assert_eq!(counter.load(Ordering::Relaxed), 8001);
    }
}

/// 1337x detail-page magnet downloads through a fixture runtime that skips only the
/// unchanged `READY_IDS` gate; nothing here activates the definition.
#[cfg(test)]
mod detail_magnet_tests {
    use super::*;
    use crate::builtin_runtime::DetailFailure;
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use serde_json::json;
    use tower::ServiceExt;

    const MAGNET: &str = "magnet:?xt=urn:btih:0123456789ABCDEF0123456789ABCDEF01234567&dn=Alpha+S01E02&tr=udp%3A%2F%2Ftracker.example%3A1337%2Fannounce";
    const ITORRENTS: &str =
        "http://itorrents.org/torrent/0123456789ABCDEF0123456789ABCDEF01234567.torrent";
    const DETAIL: &str = "/torrent/77/Alpha-S01E02/";

    fn listing() -> String {
        format!(
            "<table><tbody><tr><td class=\"coll-1 name\"><a href=\"/sub/41/0/\" class=\"icon\">c</a>\
             <a href=\"{DETAIL}\">Alpha S01E02</a></td><td class=\"coll-2 seeds\">5</td>\
             <td class=\"coll-3 leeches\">1</td><td class=\"coll-date\">Apr. 18th '11</td>\
             <td class=\"coll-4 size\">1.5 GB<span class=\"seeds\">5</span></td><td class=\"coll-5 user\"><a href=\"/user/x/\">x</a></td></tr></tbody></table>"
        )
    }
    fn page(links: &[&str]) -> String {
        let items: String = links
            .iter()
            .map(|h| format!("<li><a href=\"{}\">Get</a></li>", h.replace('&', "&amp;")))
            .collect();
        format!(
            "<div class=\"torrent-detail-page\"><ul class=\"download-links\">{items}</ul></div>"
        )
    }

    /// Loopback 1337x mirror serving listings and one mutable detail page.
    #[derive(Clone)]
    struct Mirror {
        hits: Arc<std::sync::Mutex<Vec<String>>>,
        detail: Arc<std::sync::Mutex<String>>,
    }
    impl Mirror {
        async fn serve() -> (Self, String, tokio::task::JoinHandle<()>) {
            let mirror = Self {
                hits: Default::default(),
                detail: Arc::new(std::sync::Mutex::new(page(&[MAGNET, ITORRENTS]))),
            };
            let m = mirror.clone();
            let app = axum::Router::new().fallback(move |uri: axum::http::Uri| {
                let m = m.clone();
                async move {
                    m.hits.lock().unwrap().push(uri.path().to_owned());
                    let body = if uri.path().starts_with("/torrent/") {
                        m.detail.lock().unwrap().clone()
                    } else {
                        listing()
                    };
                    axum::response::Html(body)
                }
            });
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/", l.local_addr().unwrap());
            let server = tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
            (mirror, url, server)
        }
        fn details(&self) -> usize {
            self.hits
                .lock()
                .unwrap()
                .iter()
                .filter(|p| p.starts_with("/torrent/"))
                .count()
        }
        fn total(&self) -> usize {
            self.hits.lock().unwrap().len()
        }
    }

    /// The owner's configuration values, on a loopback mirror.
    fn source(url: &str) -> Indexer {
        Indexer {
            id: "fixture-1337x".into(),
            name: "1337x".into(),
            kind: "cardigann".into(),
            url: url.into(),
            definition: Some(json!({"builtin_id":"1337x","settings":{"uploader":"",
                "primarydownloadlink":"magnet:","fallbackdownloadlink":"//itorrents.",
                "sort":"time","type":"desc","disablesort":false}})),
            ..Default::default()
        }
    }
    /// Publish a fresh fixture runtime (a new epoch) for `i`, as a saved edit would.
    async fn install(engine: &Engine, i: &Indexer, delay_ms: Option<u64>) -> Arc<Runtime> {
        let mut builtin = crate::builtin::Runtime::compile_unready(i).unwrap();
        assert_eq!(builtin.delay_ms, 3000);
        if let Some(delay_ms) = delay_ms {
            builtin.delay_ms = delay_ms;
        }
        let r = Arc::new(Engine::runtime_with(i, Some(builtin)).unwrap());
        let mut config = engine.config.write().await;
        config.indexers.retain(|x| x.id != i.id);
        config.indexers.push(i.clone());
        engine.runtime.write().await.insert(i.id.clone(), r.clone());
        r
    }
    async fn engine() -> (tempfile::TempDir, Arc<Engine>) {
        let dir = tempfile::tempdir().unwrap();
        let engine = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
        (dir, engine)
    }
    async fn get(engine: &Arc<Engine>, path: &str) -> axum::response::Response {
        crate::protocol::router()
            .with_state(AppState {
                engine: engine.clone(),
            })
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }
    async fn text(response: axum::response::Response) -> String {
        String::from_utf8(
            axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }
    /// Search through Torznab and return the item's ticket as a request path.
    async fn ticket(engine: &Arc<Engine>, id: &str) -> String {
        let key = engine.settings().await.api_key;
        let feed = text(
            get(
                engine,
                &format!("/torznab/{id}/api?t=search&q=alpha&cache=false&apikey={key}"),
            )
            .await,
        )
        .await;
        let doc = roxmltree::Document::parse(&feed).unwrap();
        let links: Vec<_> = doc
            .descendants()
            .filter(|n| n.tag_name().name() == "enclosure")
            .map(|n| url::Url::parse(n.attribute("url").unwrap()).unwrap())
            .collect();
        assert_eq!(links.len(), 1, "{feed}");
        assert!(
            !feed.contains("magnet:"),
            "magnets resolve lazily, not in feeds"
        );
        assert!(links[0].path().starts_with("/dl/"));
        format!("{}?{}", links[0].path(), links[0].query().unwrap())
    }
    async fn failure(response: axum::response::Response) -> (StatusCode, String) {
        let status = response.status();
        let body = text(response).await;
        let doc = roxmltree::Document::parse(&body).unwrap();
        let description = doc
            .root_element()
            .attribute("description")
            .unwrap()
            .to_owned();
        (status, description)
    }

    #[tokio::test]
    async fn torznab_ticket_redirects_to_the_detail_page_magnet() {
        let (mirror, url, server) = Mirror::serve().await;
        let (_dir, engine) = engine().await;
        let i = source(&url);
        install(&engine, &i, Some(0)).await;
        let dl = ticket(&engine, &i.id).await;
        // Searching fetched listings only: no detail page per result.
        assert_eq!(mirror.details(), 0);
        assert!(mirror.total() >= 1);

        let response = get(&engine, &dl).await;
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(response.headers()[header::LOCATION], MAGNET);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(text(response).await.is_empty());
        assert_eq!(*mirror.hits.lock().unwrap().last().unwrap(), DETAIL);
        assert_eq!(mirror.details(), 1);

        // Honest, fixed failures: nothing from the page or its URLs is echoed.
        for (links, expected) in [
            (vec![ITORRENTS], DetailFailure::FallbackUnsupported),
            (vec![], DetailFailure::NoMagnet),
            (
                vec!["magnet:?xt=urn:btih:0123#x", MAGNET],
                DetailFailure::InvalidMagnet,
            ),
        ] {
            *mirror.detail.lock().unwrap() = page(&links);
            let (status, description) = failure(get(&engine, &dl).await).await;
            assert_eq!(status, StatusCode::BAD_GATEWAY);
            assert_eq!(description, expected.to_string());
            assert!(!description.contains("itorrents.") && !description.contains("127.0.0.1"));
        }
        assert_eq!(mirror.details(), 4);

        // A ticketed URL that is not a detail page is refused before any request.
        let epoch = engine.runtime.read().await[&i.id].epoch;
        for target in [
            format!("{url}user/x/"),
            format!("{url}torrent/77/Alpha/?passkey=x"),
            format!("{url}torrent/77/Alpha/#x"),
        ] {
            let e = engine.download(&i.id, epoch, &target).await.unwrap_err();
            assert_eq!(
                e.downcast_ref::<DetailFailure>(),
                Some(&DetailFailure::Target)
            );
        }
        assert_eq!(mirror.details(), 4);
        server.abort();
    }

    #[tokio::test]
    async fn detail_ticket_uses_saved_cookie_solver_and_epoch_clearance() {
        use crate::builtin_runtime::{flaresolverr::Solver, Source};
        let hits = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = hits.clone();
        let origin = axum::Router::new().fallback(
            move |uri: axum::http::Uri, headers: axum::http::HeaderMap| {
                let recorded = recorded.clone();
                async move {
                    let cookie = headers
                        .get("cookie")
                        .and_then(|h| h.to_str().ok())
                        .unwrap_or("")
                        .to_owned();
                    recorded
                        .lock()
                        .unwrap()
                        .push((uri.path().to_owned(), cookie.clone()));
                    if uri.path().starts_with("/torrent/")
                        && !cookie.contains("cf_clearance=fixture-clearance")
                    {
                        (
                            StatusCode::FORBIDDEN,
                            [("cf-mitigated", "challenge")],
                            "challenge".to_owned(),
                        )
                    } else if uri.path().starts_with("/torrent/") {
                        (
                            StatusCode::OK,
                            [("content-type", "text/html")],
                            page(&[MAGNET]),
                        )
                    } else {
                        (StatusCode::OK, [("content-type", "text/html")], listing())
                    }
                }
            },
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, origin).await.unwrap() });
        let solves = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = solves.clone();
        let solver_app = axum::Router::new().route("/v1", axum::routing::post(
            move |axum::Json(body): axum::Json<serde_json::Value>| {
                let recorded = recorded.clone();
                async move {
                    recorded.lock().unwrap().push(body.clone());
                    axum::Json(json!({"status":"ok", "solution": {
                        "url":body["url"], "status":200,
                        "response":format!("<noscript><img src='/pixel'></noscript>{}", page(&[MAGNET])),
                        "userAgent":"FixtureSolver/1.0",
                        "cookies":[{"name":"cf_clearance","value":"fixture-clearance", "domain":"127.0.0.1","path":"/"}]
                    }}))
                }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let solver_url = format!("http://{}/", listener.local_addr().unwrap());
        let solver_server =
            tokio::spawn(async move { axum::serve(listener, solver_app).await.unwrap() });
        let (_dir, engine) = engine().await;
        let mut i = source(&url);
        i.cookie = "saved=fixture-private".into();
        let mut builtin = crate::builtin::Runtime::compile_unready(&i).unwrap();
        builtin.delay_ms = 0;
        let mut runtime = Engine::runtime_with(&i, Some(builtin)).unwrap();
        runtime.source =
            Some(Source::new(&i, Some(Arc::new(Solver::new(&solver_url).unwrap()))).unwrap());
        engine.config.write().await.indexers.push(i.clone());
        engine
            .runtime
            .write()
            .await
            .insert(i.id.clone(), Arc::new(runtime));
        let dl = ticket(&engine, &i.id).await;
        assert!(hits
            .lock()
            .unwrap()
            .iter()
            .all(|(path, _)| !path.starts_with("/torrent/")));
        for _ in 0..2 {
            let response = get(&engine, &dl).await;
            assert_eq!(response.status(), StatusCode::FOUND);
            assert_eq!(response.headers()[header::LOCATION], MAGNET);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
        let requests = solves.lock().unwrap();
        assert_eq!(requests.len(), 1, "second download reuses clearance");
        assert_eq!(requests[0]["cmd"], "request.get");
        assert_eq!(
            requests[0]["url"],
            format!("{}{}", url.trim_end_matches('/'), DETAIL)
        );
        assert!(!requests[0].to_string().contains("fixture-private"));
        let hits = hits.lock().unwrap();
        assert!(hits
            .iter()
            .all(|(_, cookie)| cookie.contains("saved=fixture-private")));
        assert!(hits
            .last()
            .unwrap()
            .1
            .contains("cf_clearance=fixture-clearance"));
        server.abort();
        solver_server.abort();
    }

    #[tokio::test]
    async fn edits_deletes_and_restarts_expire_detail_tickets_without_requests() {
        let (mirror, url, server) = Mirror::serve().await;
        let (dir, engine) = engine().await;
        let i = source(&url);
        install(&engine, &i, Some(0)).await;
        let expired = |response: axum::response::Response| async move {
            let (status, description) = failure(response).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert_eq!(description, "Download expired; search again");
        };
        // Edit: the same source under a new runtime retires the old ticket.
        let old = ticket(&engine, &i.id).await;
        install(&engine, &i, Some(0)).await;
        expired(get(&engine, &old).await).await;
        assert_eq!(
            get(&engine, &ticket(&engine, &i.id).await).await.status(),
            StatusCode::FOUND
        );
        // Restart: a new engine compiles new runtimes for the same source ID.
        let old = ticket(&engine, &i.id).await;
        let restarted = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
        install(&restarted, &i, Some(0)).await;
        expired(get(&restarted, &old).await).await;
        // Delete.
        let old = ticket(&engine, &i.id).await;
        engine.runtime.write().await.remove(&i.id);
        expired(get(&engine, &old).await).await;
        assert_eq!(mirror.details(), 1);
        server.abort();
    }

    #[tokio::test]
    async fn queued_detail_downloads_respect_permits_gate_and_runtime_changes() {
        let (mirror, url, server) = Mirror::serve().await;
        let (_dir, engine) = engine().await;
        let i = source(&url);
        let target = format!("{}{}", url.trim_end_matches('/'), DETAIL);
        // The pinned 3 s delay is kept: the first request finds an open gate.
        let r = install(&engine, &i, None).await;

        // All engine permits taken: the download waits and sends nothing.
        let held = engine.permits.acquire_many(8).await.unwrap();
        let mut queued = Box::pin(engine.download(&i.id, r.epoch, &target));
        assert!(futures_util::poll!(queued.as_mut()).is_pending());
        assert_eq!(mirror.total(), 0);
        drop(held);
        assert_eq!(queued.await.unwrap(), Download::Magnet(MAGNET.into()));
        assert_eq!(mirror.details(), 1);
        // The source gate now spaces the next request by the pinned delay.
        let next = *r.gate.lock().await;
        assert!(next >= Instant::now() + Duration::from_millis(2500));

        // A second download waits on that gate; cancelling it releases gate and permit.
        let mut queued = Box::pin(engine.download(&i.id, r.epoch, &target));
        assert!(futures_util::poll!(queued.as_mut()).is_pending());
        assert!(r.gate.try_lock().is_err());
        assert_eq!(engine.permits.available_permits(), 7);
        drop(queued);
        assert!(r.gate.try_lock().is_ok());
        assert_eq!(engine.permits.available_permits(), 8);
        assert_eq!(mirror.details(), 1);

        // Queued behind the gate, then the source is edited: nothing is sent.
        let r = install(&engine, &i, Some(0)).await;
        let gate = r.gate.lock().await;
        let mut queued = Box::pin(engine.download(&i.id, r.epoch, &target));
        assert!(futures_util::poll!(queued.as_mut()).is_pending());
        install(&engine, &i, Some(0)).await;
        drop(gate);
        assert!(queued.await.unwrap_err().is::<DownloadExpired>());
        assert_eq!(mirror.details(), 1);

        // A cross-origin ticket target is refused before queueing.
        let r = engine.runtime.read().await[&i.id].clone();
        let other = "http://127.0.0.1:9/torrent/77/Alpha-S01E02/";
        assert!(engine.download(&i.id, r.epoch, other).await.is_err());
        assert_eq!(mirror.total(), 1);
        server.abort();
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    use crate::store::{DurabilityUnconfirmed, SaveHooks};
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use futures_util::future::BoxFuture;
    use serde_json::json;
    use tower::ServiceExt;

    const UNCONFIRMED: &str =
        "Configuration was applied, but disk durability could not be confirmed";

    /// Every managed mutation. Startup setters run as direct calls; the rest go through
    /// the real management API so a dropped request is a dropped HTTP handler.
    #[derive(Clone, Copy, Debug)]
    enum Op {
        Create,
        Patch,
        Delete,
        Settings,
        Rotate,
        Port,
        PublicUrl,
    }
    const OPS: [Op; 7] = [
        Op::Create,
        Op::Patch,
        Op::Delete,
        Op::Settings,
        Op::Rotate,
        Op::Port,
        Op::PublicUrl,
    ];

    struct Fixture {
        _dir: tempfile::TempDir,
        path: PathBuf,
        engine: Arc<Engine>,
        seeded: String,
        epoch: u64,
        before: Config,
    }

    fn feed(name: &str) -> Indexer {
        Indexer {
            name: name.into(),
            url: "http://127.0.0.1:9/feed".into(),
            ..Default::default()
        }
    }

    async fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let engine = Arc::new(Engine::new(path.clone()).await.unwrap());
        let seeded = engine.put_indexer(feed("Seeded"), false).await.unwrap().id;
        let epoch = engine.runtime.read().await[&seeded].epoch;
        let before = engine.config.read().await.clone();
        Fixture {
            _dir: dir,
            path,
            engine,
            seeded,
            epoch,
            before,
        }
    }

    impl Engine {
        fn hook_next_save(&self, hooks: SaveHooks) {
            *self.test.save.lock().unwrap() = Some(hooks);
        }
        async fn wait_finished(&self, count: u64) {
            self.test
                .finished
                .subscribe()
                .wait_for(|n| *n >= count)
                .await
                .unwrap();
        }
        fn finished(&self) -> u64 {
            *self.test.finished.borrow()
        }
    }

    /// Wait for a save hook; a caller finishing first never reached it, so fail, not hang.
    async fn reached<T: std::fmt::Debug>(
        signal: tokio::sync::oneshot::Receiver<()>,
        caller: &mut BoxFuture<'static, T>,
    ) {
        tokio::select! {
            biased;
            hooked = signal => hooked.expect("save hook dropped unsignalled"),
            done = caller => panic!("change finished before its save hook: {done:?}"),
        }
    }

    fn value<T: Serialize>(v: &T) -> serde_json::Value {
        serde_json::to_value(v).unwrap()
    }

    async fn api(
        engine: &Arc<Engine>,
        method: &str,
        uri: String,
        body: Option<serde_json::Value>,
    ) -> Result<(), String> {
        let app = crate::api::router().with_state(AppState {
            engine: engine.clone(),
        });
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "localhost")
            .header("origin", "http://localhost");
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let request = request
            .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        if status.is_success() {
            Ok(())
        } else {
            assert_eq!(status, StatusCode::BAD_REQUEST);
            Err(body["error"].as_str().unwrap().to_owned())
        }
    }

    fn start(op: Op, f: &Fixture) -> BoxFuture<'static, Result<(), String>> {
        let e = f.engine.clone();
        let seeded = f.seeded.clone();
        Box::pin(async move {
            match op {
                Op::Create => {
                    api(
                        &e,
                        "POST",
                        "/api/indexers".into(),
                        Some(value(&feed("Created"))),
                    )
                    .await
                }
                Op::Patch => {
                    api(
                        &e,
                        "PATCH",
                        format!("/api/indexers/{seeded}"),
                        Some(json!({"name":"Patched"})),
                    )
                    .await
                }
                Op::Delete => api(&e, "DELETE", format!("/api/indexers/{seeded}"), None).await,
                Op::Settings => {
                    api(
                        &e,
                        "PATCH",
                        "/api/settings".into(),
                        Some(json!({"cache_ttl_seconds":7})),
                    )
                    .await
                }
                Op::Rotate => api(&e, "POST", "/api/settings/rotate-key".into(), None).await,
                Op::Port => e.set_port(18080).await.map_err(|e| e.to_string()),
                Op::PublicUrl => e
                    .set_public_url("https://rackett.example/")
                    .await
                    .map_err(|e| e.to_string()),
            }
        })
    }

    /// The operation's own change is present in `c`.
    fn applied(op: Op, f: &Fixture, c: &Config) -> bool {
        let seeded = c.indexers.iter().find(|i| i.id == f.seeded);
        match op {
            Op::Create => {
                c.indexers.len() == f.before.indexers.len() + 1
                    && c.indexers.iter().any(|i| i.name == "Created")
            }
            Op::Patch => seeded.is_some_and(|i| i.name == "Patched"),
            Op::Delete => seeded.is_none(),
            Op::Settings => c.settings.cache_ttl_seconds == 7,
            Op::Rotate => c.settings.api_key != f.before.settings.api_key,
            Op::Port => c.settings.port == 18080,
            Op::PublicUrl => c.settings.public_url.as_deref() == Some("https://rackett.example"),
        }
    }

    /// Memory, compiled runtimes, the file and a fresh engine all agree; returns that state.
    async fn coherent(f: &Fixture) -> Config {
        let memory = f.engine.config.read().await.clone();
        {
            let runtime = f.engine.runtime.read().await;
            assert_eq!(runtime.len(), memory.indexers.len());
            for i in &memory.indexers {
                assert_eq!(value(&runtime[&i.id].indexer), value(i));
            }
        }
        assert_eq!(value(&store::load(&f.path).await.unwrap()), value(&memory));
        let restarted = Engine::new(f.path.clone()).await.unwrap();
        assert_eq!(value(&*restarted.config.read().await), value(&memory));
        let entries: Vec<_> = std::fs::read_dir(f.path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries, ["config.json"], "no temporary files remain");
        memory
    }

    /// Effects outside the config: runtime epochs and the protocol key.
    async fn assert_live_effects(op: Op, f: &Fixture, c: &Config) {
        let runtime = f
            .engine
            .runtime
            .read()
            .await
            .get(&f.seeded)
            .map(|r| r.epoch);
        match op {
            Op::Patch => assert!(runtime.is_some_and(|epoch| epoch != f.epoch)),
            Op::Delete => assert_eq!(runtime, None),
            _ => assert_eq!(runtime, Some(f.epoch)),
        }
        let app = crate::protocol::router().with_state(AppState {
            engine: f.engine.clone(),
        });
        let caps = |key: &str| {
            Request::builder()
                .uri(format!(
                    "/api/v2.0/indexers/all/results/torznab?t=caps&apikey={key}"
                ))
                .body(Body::empty())
                .unwrap()
        };
        let current = app
            .clone()
            .oneshot(caps(&c.settings.api_key))
            .await
            .unwrap();
        assert_eq!(current.status(), StatusCode::OK);
        if matches!(op, Op::Rotate) {
            let old = app.oneshot(caps(&f.before.settings.api_key)).await.unwrap();
            assert_eq!(old.status(), StatusCode::UNAUTHORIZED);
        }
    }

    fn assert_idle(engine: &Engine) {
        assert!(engine.write.try_lock().is_ok(), "writer released");
        assert_eq!(engine.admission.available_permits(), MANAGEMENT_QUEUE);
    }

    #[tokio::test]
    async fn dropped_callers_paused_before_rename_still_commit_in_order() {
        for op in OPS {
            let f = fixture().await;
            let (entered_tx, entered) = tokio::sync::oneshot::channel();
            let (release, release_rx) = std::sync::mpsc::channel();
            f.engine.hook_next_save(SaveHooks {
                pause_before_rename: Some((entered_tx, release_rx)),
                ..Default::default()
            });
            let mut caller = start(op, &f);
            assert!(futures_util::poll!(&mut caller).is_pending());
            assert_eq!(
                f.engine.admission.available_permits(),
                MANAGEMENT_QUEUE - 1,
                "{op:?} admitted on first poll"
            );
            reached(entered, &mut caller).await;
            drop(caller);
            assert!(
                f.engine.write.try_lock().is_err(),
                "{op:?} still owns the writer"
            );
            let mut second = Box::pin(
                f.engine
                    .update_settings(json!({"request_timeout_seconds":9})),
            );
            assert!(futures_util::poll!(&mut second).is_pending());
            assert_eq!(value(&*f.engine.config.read().await), value(&f.before));
            assert_eq!(
                value(&store::load(&f.path).await.unwrap()),
                value(&f.before)
            );
            release.send(()).unwrap();
            // The queued update ran after the detached one, so it saw and kept its change.
            second.await.unwrap();
            let c = coherent(&f).await;
            assert!(applied(op, &f, &c), "{op:?}");
            assert_eq!(c.settings.request_timeout_seconds, 9);
            assert_live_effects(op, &f, &c).await;
            assert_idle(&f.engine);
        }
    }

    #[tokio::test]
    async fn dropped_callers_paused_after_save_still_apply_runtime_and_memory() {
        for op in OPS {
            let f = fixture().await;
            let (committed_tx, committed) = tokio::sync::oneshot::channel();
            f.engine.hook_next_save(SaveHooks {
                committed: Some(committed_tx),
                ..Default::default()
            });
            // Readers held across the save block every post-save write.
            let config = f.engine.config.read().await;
            let runtime = f.engine.runtime.read().await;
            let mut caller = start(op, &f);
            assert!(futures_util::poll!(&mut caller).is_pending());
            assert_eq!(f.engine.admission.available_permits(), MANAGEMENT_QUEUE - 1);
            reached(committed, &mut caller).await;
            drop(caller);
            assert!(
                applied(op, &f, &store::load(&f.path).await.unwrap()),
                "{op:?}"
            );
            assert_eq!(value(&*config), value(&f.before));
            let mut second = Box::pin(
                f.engine
                    .update_settings(json!({"request_timeout_seconds":9})),
            );
            assert!(futures_util::poll!(&mut second).is_pending());
            drop(runtime);
            drop(config);
            second.await.unwrap();
            let c = coherent(&f).await;
            assert!(applied(op, &f, &c), "{op:?}");
            assert_eq!(c.settings.request_timeout_seconds, 9);
            assert_live_effects(op, &f, &c).await;
            assert_idle(&f.engine);
        }
    }

    #[tokio::test]
    async fn failure_before_rename_changes_nothing() {
        for op in OPS {
            let f = fixture().await;
            f.engine.hook_next_save(SaveHooks {
                fail_before_rename: true,
                ..Default::default()
            });
            let error = start(op, &f).await.unwrap_err();
            assert_eq!(error, "injected failure before rename", "{op:?}");
            let c = coherent(&f).await;
            assert_eq!(value(&c), value(&f.before), "{op:?}");
            assert_eq!(f.engine.runtime.read().await[&f.seeded].epoch, f.epoch);
            assert_idle(&f.engine);
            assert_eq!(
                f.engine.finished(),
                2,
                "seed and failed change both finished"
            );
        }
    }

    #[tokio::test]
    async fn directory_sync_failure_applies_then_reports_unconfirmed() {
        for op in OPS {
            let f = fixture().await;
            f.engine.hook_next_save(SaveHooks {
                fail_directory_sync: true,
                ..Default::default()
            });
            // Created sources and rotated keys are live although the caller sees an error.
            assert_eq!(start(op, &f).await.unwrap_err(), UNCONFIRMED, "{op:?}");
            let c = coherent(&f).await;
            assert!(applied(op, &f, &c), "{op:?}");
            assert_live_effects(op, &f, &c).await;
            assert_idle(&f.engine);
        }
        let f = fixture().await;
        f.engine.hook_next_save(SaveHooks {
            fail_directory_sync: true,
            ..Default::default()
        });
        let error = f.engine.rotate_key().await.unwrap_err();
        assert!(error.is::<DurabilityUnconfirmed>());
        assert_ne!(f.engine.settings().await.api_key, f.before.settings.api_key);
    }

    #[tokio::test]
    async fn worker_panic_after_rename_is_reported_unconfirmed_and_applied() {
        let f = fixture().await;
        f.engine.hook_next_save(SaveHooks {
            panic_after_rename: true,
            ..Default::default()
        });
        let error = f.engine.set_port(18080).await.unwrap_err();
        assert!(error.is::<DurabilityUnconfirmed>());
        let c = coherent(&f).await;
        assert!(applied(Op::Port, &f, &c));
        assert_idle(&f.engine);
    }

    #[tokio::test]
    async fn admission_is_bounded_and_unpolled_calls_do_nothing() {
        let f = fixture().await;
        // Creating and dropping an unpolled call neither admits nor changes anything.
        drop(f.engine.set_port(18080));
        drop(start(Op::Rotate, &f));
        assert_idle(&f.engine);
        let (entered_tx, entered) = tokio::sync::oneshot::channel();
        let (release, release_rx) = std::sync::mpsc::channel::<()>();
        f.engine.hook_next_save(SaveHooks {
            pause_before_rename: Some((entered_tx, release_rx)),
            ..Default::default()
        });
        let mut callers = Vec::new();
        for ttl in 0..MANAGEMENT_QUEUE as u64 {
            let e = f.engine.clone();
            let mut caller: BoxFuture<'static, Result<Settings>> =
                Box::pin(async move { e.update_settings(json!({"cache_ttl_seconds":ttl})).await });
            assert!(futures_util::poll!(&mut caller).is_pending());
            callers.push(caller);
        }
        reached(entered, &mut callers[0]).await;
        assert_eq!(f.engine.admission.available_permits(), 0);
        let rejected = f.engine.rotate_key().await.unwrap_err();
        assert!(rejected.is::<ManagementBusy>());
        assert_eq!(
            start(Op::Patch, &f).await.unwrap_err(),
            "Too many configuration changes in progress; retry shortly"
        );
        assert_eq!(f.engine.admission.available_permits(), 0);
        // Every admitted change finishes, in admission order, with all callers gone.
        drop(callers);
        drop(release);
        let seeded = f.engine.finished();
        f.engine
            .wait_finished(seeded + MANAGEMENT_QUEUE as u64)
            .await;
        assert_idle(&f.engine);
        let c = coherent(&f).await;
        assert_eq!(c.settings.cache_ttl_seconds, MANAGEMENT_QUEUE as u64 - 1);
        assert_eq!(c.settings.api_key, f.before.settings.api_key);
        assert_eq!(c.indexers[0].name, "Seeded");
        f.engine.rotate_key().await.unwrap();
    }

    #[tokio::test]
    async fn panicking_change_releases_the_writer_and_disables_changes() {
        let f = fixture().await;
        let error = f
            .engine
            .transact(|_| async move {
                if f64::NAN.is_nan() {
                    panic!("injected panic inside a change");
                }
                Ok(())
            })
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), OUTCOME_UNKNOWN);
        assert_idle(&f.engine);
        assert_eq!(
            f.engine.set_port(18080).await.unwrap_err().to_string(),
            RESTART_REQUIRED
        );
        assert_eq!(value(&coherent(&f).await), value(&f.before));
    }
}

#[cfg(test)]
mod aggregate_streaming_tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use serde_json::json;
    use tower::ServiceExt;
    async fn engine() -> (tempfile::TempDir, Arc<Engine>) {
        let dir = tempfile::tempdir().unwrap();
        let engine = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
        (dir, engine)
    }
    async fn get(engine: &Arc<Engine>, path: &str) -> axum::response::Response {
        crate::protocol::router()
            .with_state(AppState {
                engine: engine.clone(),
            })
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }
    async fn text(response: axum::response::Response) -> String {
        String::from_utf8(
            axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }
    /// A loopback feed with one magnet release that answers after `delay_ms`, counting requests.
    async fn feed_fixture(delay_ms: u64) -> (String, Arc<AtomicU64>, tokio::task::JoinHandle<()>) {
        use axum::{routing::get, Router};
        let hits = Arc::new(AtomicU64::new(0));
        let seen = hits.clone();
        let app = Router::new().fallback(get(move || {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                "<rss version=\"2.0\"><channel><title>Fixture</title><item><title>Alpha</title><guid>alpha</guid><link>magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&amp;dn=Alpha</link></item></channel></rss>"
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/feed", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, hits, server)
    }
    fn feed_source(name: &str, url: String, categories: Vec<u32>) -> Indexer {
        Indexer {
            name: name.into(),
            url,
            request_delay_ms: 0,
            categories,
            ..Default::default()
        }
    }
    fn query(pairs: &[(&str, &str)]) -> SearchQuery {
        SearchQuery {
            params: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }
    #[tokio::test]
    async fn expired_results_answer_at_once_while_a_refresh_runs_behind() {
        let (url, hits, server) = feed_fixture(0).await;
        let (_dir, engine) = engine().await;
        engine
            .update_settings(json!({"cache_ttl_seconds":1,"cache_stale_seconds":600}))
            .await
            .unwrap();
        let i = engine
            .put_indexer(feed_source("Fixture", url, vec![]), false)
            .await
            .unwrap();
        let q = query(&[("q", "alpha")]);
        let first = engine.search(vec![i.id.clone()], q.clone()).await;
        assert_eq!((first.results.len(), first.cached, first.stale), (1, 0, 0));
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let served = engine.search(vec![i.id.clone()], q.clone()).await;
        assert_eq!(
            (served.results.len(), served.cached, served.stale),
            (1, 1, 1)
        );
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(hits.load(Ordering::Relaxed), 2, "one background refresh");
        let fresh = engine.search(vec![i.id], q).await;
        assert_eq!((fresh.results.len(), fresh.cached, fresh.stale), (1, 1, 0));
        assert_eq!(hits.load(Ordering::Relaxed), 2);
        server.abort();
    }
    #[tokio::test]
    async fn stale_window_off_keeps_the_old_expiry() {
        let (url, hits, server) = feed_fixture(0).await;
        let (_dir, engine) = engine().await;
        engine
            .update_settings(json!({"cache_ttl_seconds":1,"cache_stale_seconds":0}))
            .await
            .unwrap();
        let i = engine
            .put_indexer(feed_source("Fixture", url, vec![]), false)
            .await
            .unwrap();
        let q = query(&[("q", "alpha")]);
        engine.search(vec![i.id.clone()], q.clone()).await;
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let again = engine.search(vec![i.id], q).await;
        assert_eq!(
            (again.cached, again.stale, hits.load(Ordering::Relaxed)),
            (0, 0, 2)
        );
        assert!(engine
            .update_settings(json!({"cache_stale_seconds":604_801}))
            .await
            .is_err());
        server.abort();
    }
    #[tokio::test]
    async fn maxwait_answers_with_fast_sources_and_lets_slow_ones_fill_the_cache() {
        let (fast_url, fast_hits, fast_server) = feed_fixture(0).await;
        let (slow_url, slow_hits, slow_server) = feed_fixture(1500).await;
        let (_dir, engine) = engine().await;
        engine
            .put_indexer(feed_source("Fast", fast_url, vec![]), false)
            .await
            .unwrap();
        engine
            .put_indexer(feed_source("Slow", slow_url, vec![]), false)
            .await
            .unwrap();
        let started = Instant::now();
        let partial = engine
            .search(
                vec!["all".into()],
                query(&[("q", "alpha"), ("maxwait", "0.5")]),
            )
            .await;
        assert!(
            started.elapsed() < Duration::from_millis(1400),
            "answered at maxwait"
        );
        assert_eq!(partial.results.len(), 1);
        assert!(partial.errors.is_empty());
        assert_eq!(
            partial
                .pending
                .iter()
                .map(|p| p.indexer.as_str())
                .collect::<Vec<_>>(),
            ["Slow"]
        );
        // The Torznab feed carries the same information for clients that poll.
        let key = engine.settings().await.api_key;
        let feed = text(
            get(
                &engine,
                &format!("/torznab/all/api?t=search&q=alpha&maxwait=0.1&apikey={key}"),
            )
            .await,
        )
        .await;
        assert!(feed.contains("<rackett:pending id=\""), "{feed}");
        assert!(feed.contains("name=\"Slow\""), "{feed}");
        assert!(
            feed.contains("<item>"),
            "fast results are in the feed: {feed}"
        );
        tokio::time::sleep(Duration::from_millis(1700)).await;
        let complete = engine
            .search(
                vec!["all".into()],
                query(&[("q", "alpha"), ("maxwait", "0.5")]),
            )
            .await;
        assert_eq!((complete.results.len(), complete.pending.len()), (2, 0));
        assert_eq!(complete.cached, 2, "the slow fetch finished and was cached");
        // The Torznab call above is a different cache key (it carries `t`), so each source saw
        // one request per key and none for the repeated engine query.
        assert_eq!(
            (
                fast_hits.load(Ordering::Relaxed),
                slow_hits.load(Ordering::Relaxed)
            ),
            (2, 2)
        );
        fast_server.abort();
        slow_server.abort();
    }
    #[tokio::test]
    async fn only_pending_sources_is_an_empty_feed_not_a_failure() {
        let (slow_url, _hits, slow_server) = feed_fixture(1500).await;
        let (_dir, engine) = engine().await;
        engine
            .put_indexer(feed_source("Slow", slow_url, vec![]), false)
            .await
            .unwrap();
        let key = engine.settings().await.api_key;
        let response = get(
            &engine,
            &format!("/torznab/all/api?t=search&q=alpha&maxwait=0.1&apikey={key}"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let feed = text(response).await;
        assert!(
            feed.contains("name=\"Slow\"") && !feed.contains("<item>"),
            "{feed}"
        );
        slow_server.abort();
    }
    #[tokio::test]
    async fn torznab_aggregate_keeps_category_skipping_after_capability_filtering() {
        let (url, hits, server) = feed_fixture(0).await;
        let (_dir, engine) = engine().await;
        let source = engine
            .put_indexer(feed_source("Anime", url, vec![5070]), false)
            .await
            .unwrap();
        let key = engine.settings().await.api_key;
        for route in [
            "/torznab/all/api",
            "/api/v2.0/indexers/all/results/torznab/api",
        ] {
            let response = get(
                &engine,
                &format!("{route}?t=search&q=alpha&cat=2000&apikey={key}"),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            assert!(!text(response).await.contains("<item>"));
            assert_eq!(hits.load(Ordering::Relaxed), 0);
        }
        let response = get(
            &engine,
            &format!("/torznab/all/api?t=search&q=alpha&cat=5000&apikey={key}"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(text(response).await.contains("<item>"));
        assert_eq!(hits.load(Ordering::Relaxed), 1);
        // Explicit queries keep their existing dispatch contract, even when the
        // returned rows are subsequently excluded by the requested category.
        let response = get(
            &engine,
            &format!(
                "/torznab/{}/api?t=search&q=alpha&cat=2000&apikey={key}",
                source.id
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(hits.load(Ordering::Relaxed), 2);
        server.abort();
    }
    #[tokio::test]
    async fn aggregate_skips_sources_that_cannot_carry_the_requested_category() {
        let (url, hits, server) = feed_fixture(0).await;
        let (_dir, engine) = engine().await;
        engine
            .put_indexer(feed_source("Anime", url, vec![5070]), false)
            .await
            .unwrap();
        let movies = engine
            .search(
                vec!["all".into()],
                query(&[("q", "alpha"), ("cat", "2000")]),
            )
            .await;
        assert_eq!((movies.results.len(), hits.load(Ordering::Relaxed)), (0, 0));
        assert_eq!(movies.skipped, ["Anime"]);
        let tv = engine
            .search(
                vec!["all".into()],
                query(&[("q", "alpha"), ("cat", "5000")]),
            )
            .await;
        assert_eq!((tv.results.len(), hits.load(Ordering::Relaxed)), (1, 1));
        assert!(tv.skipped.is_empty());
        let exact = engine
            .search(
                vec!["all".into()],
                query(&[("q", "alpha"), ("cat", "5070")]),
            )
            .await;
        // A child category is its own cache key but is still served by the parent's source.
        assert_eq!((exact.results.len(), hits.load(Ordering::Relaxed)), (1, 2));
        server.abort();
    }
}
