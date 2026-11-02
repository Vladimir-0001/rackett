//! Bounded supplied-cookie authentication; catalog admission belongs to builtin.rs.
//! Response cookies are bounded and source-scoped. Declared `search.headers` are rendered
//! once per session and sent as pinned Cardigann does (docs/session-search-headers.md).
use crate::{cardigann::Node, cardigann_exec::request::Charset};
use anyhow::{anyhow, bail, Result};
use futures_util::StreamExt;
use reqwest::header::{CONTENT_TYPE, COOKIE, LOCATION};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, Weak},
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, OwnedMutexGuard, OwnedSemaphorePermit, Semaphore};

/// `login.test`. Pinned `TestLogin` (`CardigannIndexer.cs:892-949`) skips validation
/// when the block is absent and checks the page only when a selector is declared.
#[derive(Clone)]
pub(super) struct LoginTest {
    pub path: String,
    pub selector: Option<String>,
}
/// Complete, deliberately narrow login schema; never serializes credentials.
pub struct CookiePlan {
    test: Option<LoginTest>,
    // Aidoru's pinned search endpoint returns only the result table. Its login
    // marker is a torrent link on the home page, absent on an empty search.
    aidoru_fragment: bool,
    user_agent: Option<String>,
    /// `search.headers` other than the bound User-Agent, until `create` renders them.
    declared: Vec<DeclaredHeader>,
    /// Values of the settings `declared` references (`with_settings`).
    header_settings: std::collections::BTreeMap<String, String>,
    /// The rendered `declared` headers, in definition order.
    headers: Vec<(String, String)>,
    /// Definition encoding for search, test and login pages; it wins over headers.
    pub(super) charset: Charset,
    /// Definition `certificates`, trusted only for the session's own origin host.
    pub(super) certificates: Option<crate::pinned_tls::Pins>,
}

#[cfg(test)]
#[path = "auth_sessions.rs"]
mod session_tests;
/// Whether the search block or any search path declares `followredirect`. Sessions
/// treat a search redirect as login expiry, so these sources stay unsupported.
pub(crate) fn search_follows_redirects(root: &crate::cardigann::Node) -> bool {
    root.get("search").is_some_and(|s| {
        s.get("followredirect").is_some()
            || s.get("paths")
                .and_then(crate::cardigann::Node::items)
                .is_some_and(|paths| paths.iter().any(|p| p.get("followredirect").is_some()))
    })
}
fn exact_keys(node: &Node, expected: &[&str]) -> Result<()> {
    let pairs = node
        .pairs()
        .ok_or_else(|| anyhow!("Cookie auth requires a mapping"))?;
    let keys: BTreeSet<_> = pairs.iter().filter_map(|(k, _)| k.text()).collect();
    if pairs.len() != expected.len() || keys != expected.iter().copied().collect() {
        bail!("Unsupported cookie authentication schema");
    }
    Ok(())
}
// The offline AST intentionally preserves duplicate keys. First-match lookup is
// safe only after every mapping in this definition has been checked.
pub(super) fn unique_mappings(node: &Node) -> Result<()> {
    if let Some(pairs) = node.pairs() {
        let mut keys = BTreeSet::new();
        for (key, value) in pairs {
            if !key.text().is_some_and(|key| keys.insert(key)) {
                bail!("Ambiguous cookie authentication definition");
            }
            unique_mappings(value)?;
        }
    } else if let Some(items) = node.items() {
        for item in items {
            unique_mappings(item)?;
        }
    }
    Ok(())
}
impl CookiePlan {
    pub fn compile(root: &Node) -> Result<Self> {
        unique_mappings(root)?;
        let login = root
            .get("login")
            .ok_or_else(|| anyhow!("Missing cookie login"))?;
        if login.get("test").is_some() {
            exact_keys(login, &["method", "inputs", "test"])?;
        } else {
            exact_keys(login, &["method", "inputs"])?;
        }
        if login.string("method") != Some("cookie") {
            bail!("Only explicit cookie login is modeled");
        }
        let inputs = login.get("inputs").unwrap();
        exact_keys(inputs, &["cookie"])?;
        if inputs.string("cookie") != Some("{{ .Config.cookie }}") {
            bail!("Unsupported cookie input template");
        }
        let test = if let Some(test) = login.get("test") {
            if test.get("selector").is_some() {
                exact_keys(test, &["path", "selector"])?;
            } else {
                exact_keys(test, &["path"])?;
            }
            let path = test
                .string("path")
                .ok_or_else(|| anyhow!("Invalid cookie test path"))?;
            let selector = test
                .get("selector")
                .map(|s| {
                    s.text()
                        .ok_or_else(|| anyhow!("Invalid cookie test selector"))
                })
                .transpose()?;
            if path.is_empty()
                || path.len() > 2048
                || path.contains(['{', '}', '\\'])
                || path.chars().any(char::is_control)
                || selector.is_some_and(|selector| {
                    selector.is_empty()
                        || selector.len() > 1024
                        || scraper::Selector::parse(selector).is_err()
                })
            {
                bail!("Unsupported cookie test path or selector");
            }
            Some(LoginTest {
                path: path.into(),
                selector: selector.map(Into::into),
            })
        } else {
            None
        };
        if root.get("followredirect").is_some()
            || search_follows_redirects(root)
            || root
                .get("download")
                .and_then(|s| s.get("headers"))
                .is_some()
        {
            bail!("Cookie auth transport semantics are not modeled");
        }
        let charset = Charset::from_definition(root)
            .map_err(|_| anyhow!("Cookie auth transport semantics are not modeled"))?;
        let settings = root
            .get("settings")
            .and_then(Node::items)
            .unwrap_or_default();
        let cookies: Vec<_> = settings
            .iter()
            .filter(|s| s.string("name") == Some("cookie"))
            .collect();
        if cookies.len() != 1
            || ![Some("text"), Some("password")].contains(&cookies[0].string("type"))
        {
            bail!("Cookie login requires one secret cookie setting");
        }
        let search = root
            .get("search")
            .ok_or_else(|| anyhow!("Missing cookie search"))?;
        let (agent, declared) = compile_search_headers(root)?;
        let user_agent = agent.then(String::new);
        // HTML GET or plan-encoded form POST searches (`method: post`).
        let method_ok = |m: &str| m.eq_ignore_ascii_case("get") || m.eq_ignore_ascii_case("post");
        // Search responses may be HTML (default), JSON or XML; RequestPlan owns the rest
        // of the response semantics.
        let response_ok = |n: &Node| {
            n.get("response").is_none_or(|r| {
                r.string("type")
                    .is_none_or(|t| ["html", "json", "xml"].contains(&t))
            })
        };
        if !response_ok(search)
            || search.string("method").is_some_and(|m| !method_ok(m))
            || search
                .get("paths")
                .and_then(Node::items)
                .unwrap_or_default()
                .iter()
                .any(|p| !response_ok(p) || p.string("method").is_some_and(|m| !method_ok(m)))
        {
            bail!("Cookie search requires plain HTML GET or form POST paths");
        }
        let aidoru_fragment = root.string("id") == Some("aidoruonline");
        if aidoru_fragment {
            let paths = search
                .get("paths")
                .and_then(Node::items)
                .ok_or_else(|| anyhow!("Unsupported AJAX cookie search schema"))?;
            let rows = search
                .get("rows")
                .ok_or_else(|| anyhow!("Unsupported AJAX cookie search schema"))?;
            exact_keys(rows, &["selector"])?;
            if paths.len() != 1
                || paths[0].string("path") != Some("get_ttable.php")
                || rows.string("selector")
                    != Some("table.ttable_headinner tr.t-row:has(a[href^=\"download.php?id=\"])")
                || test.as_ref().is_none_or(|t| {
                    t.path != "/"
                        || t.selector.as_deref() != Some("a[href^=\"torrents-details.php?id=\"]")
                })
            {
                bail!("Unsupported AJAX cookie search schema");
            }
            exact_keys(&paths[0], &["path"])?;
        }
        Ok(Self {
            test,
            aidoru_fragment,
            charset,
            user_agent,
            declared,
            header_settings: Default::default(),
            headers: Vec::new(),
            certificates: crate::pinned_tls::Pins::compile(root)
                .map_err(|_| anyhow!("Cookie auth transport semantics are not modeled"))?,
        })
    }
    /// Only the source-declared single header can be configured. No value is echoed.
    pub fn with_user_agent(mut self, value: Option<&str>) -> Result<Self> {
        if self.user_agent.is_some() {
            let value =
                value.ok_or_else(|| anyhow!("This preset requires a User-Agent setting"))?;
            if value.trim().is_empty()
                || value.len() > 4096
                || !value.bytes().all(|b| (0x20..=0x7e).contains(&b))
            {
                bail!("Invalid cookie User-Agent setting");
            }
            self.user_agent = Some(value.into());
        }
        Ok(self)
    }
    /// Values for the text settings that declared search headers render. Only the
    /// referenced settings are kept; none is ever echoed in an error.
    pub fn with_settings(
        mut self,
        settings: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<Self> {
        for name in referenced_settings(&self.declared) {
            if let Some(value) = settings.get(&name).and_then(serde_json::Value::as_str) {
                self.header_settings.insert(name, value.to_owned());
            }
        }
        Ok(self)
    }
}

/// A part of a declared header value: literal text or `{{ .Config.<name> }}`.
#[derive(Clone, PartialEq, Debug)]
enum Part {
    Text(String),
    Setting(String),
}
/// One `search.headers` entry other than the bound User-Agent.
#[derive(Clone, Debug)]
pub(super) struct DeclaredHeader {
    name: String,
    parts: Vec<Part>,
}
/// Headers no definition may set on a session request: framing, hop-by-hop and
/// proxy headers, and `Content-Type`, which pinned HttpWebClient2 drops from custom
/// headers unless it sends a raw body (`HttpWebClient2.cs:126-135`).
const FORBIDDEN_HEADERS: &[&str] = &[
    "host",
    "connection",
    "content-length",
    "content-type",
    "expect",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];
fn visible_ascii(value: &str) -> bool {
    value.len() <= 4096 && value.bytes().all(|b| (0x20..=0x7e).contains(&b))
}
/// Literal text and `{{ .Config.<name> }}` references; any other template is rejected.
fn template_parts(text: &str) -> Result<Vec<Part>> {
    let unsupported = || anyhow!("Unsupported session header template");
    let mut parts = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let (literal, tail) = rest.split_once("{{").unwrap_or((rest, ""));
        if literal.contains("}}") || !visible_ascii(literal) {
            return Err(unsupported());
        }
        if !literal.is_empty() {
            parts.push(Part::Text(literal.to_owned()));
        }
        if literal.len() == rest.len() {
            break;
        }
        let (name, tail) = tail
            .strip_prefix(" .Config.")
            .and_then(|t| t.split_once(" }}"))
            .ok_or_else(unsupported)?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(unsupported());
        }
        parts.push(Part::Setting(name.to_owned()));
        rest = tail;
    }
    Ok(parts)
}
fn referenced_settings(declared: &[DeclaredHeader]) -> BTreeSet<String> {
    declared
        .iter()
        .flat_map(|h| &h.parts)
        .filter_map(|p| match p {
            Part::Setting(name) if name != "sitelink" => Some(name.clone()),
            _ => None,
        })
        .collect()
}
/// `search.headers`, which pinned Cardigann also sends with login, test and download
/// requests (`Login.Headers ?? Search.Headers`, `Download.Headers ?? Search.Headers`).
/// Returns whether `User-Agent: {{ .Config.useragent }}` is declared, and the rest.
///
/// Admitted values: literal text, `{{ .Config.sitelink }}`, and declared text settings
/// other than `cookie` and `useragent`. `Cookie` must be literal cookie-pairs and
/// `Referer` must start with the configured site link.
pub(super) fn compile_search_headers(root: &Node) -> Result<(bool, Vec<DeclaredHeader>)> {
    let Some(headers) = root.get("search").and_then(|s| s.get("headers")) else {
        return Ok((false, Vec::new()));
    };
    let unsupported = || anyhow!("Unsupported session header declaration");
    let settings = root
        .get("settings")
        .and_then(Node::items)
        .unwrap_or_default();
    let declared_text = |name: &str| {
        let matching: Vec<_> = settings
            .iter()
            .filter(|s| s.string("name") == Some(name))
            .collect();
        matching.len() == 1 && matching[0].string("type") == Some("text")
    };
    let pairs = headers.pairs().ok_or_else(unsupported)?;
    if pairs.len() > 16 {
        return Err(unsupported());
    }
    let mut names = BTreeSet::new();
    let mut agent = false;
    let mut declared = Vec::new();
    for (key, value) in pairs {
        let name = key.text().ok_or_else(unsupported)?;
        let lower = name.to_ascii_lowercase();
        let values = value.items().ok_or_else(unsupported)?;
        // Pinned ParseCustomHeaders sends only the first value (`CardigannIndexer.cs:2202-2205`);
        // extra values are rejected rather than silently dropped.
        let text = match values {
            [only] => only.text().ok_or_else(unsupported)?,
            _ => return Err(unsupported()),
        };
        if !names.insert(lower.clone())
            || reqwest::header::HeaderName::from_bytes(name.as_bytes()).is_err()
            || FORBIDDEN_HEADERS.contains(&lower.as_str())
        {
            return Err(unsupported());
        }
        if lower == "user-agent" {
            if text != "{{ .Config.useragent }}" || !declared_text("useragent") {
                bail!("Unsupported cookie User-Agent declaration");
            }
            agent = true;
            continue;
        }
        let parts = template_parts(text)?;
        let valid = match lower.as_str() {
            "cookie" => match parts.as_slice() {
                [Part::Text(text)] => cookie_pairs(text).is_ok(),
                _ => false,
            },
            "referer" => {
                parts.first() == Some(&Part::Setting("sitelink".into()))
                    && parts[1..].iter().all(|p| matches!(p, Part::Text(_)))
            }
            _ => !parts.is_empty(),
        } && parts.iter().all(|p| match p {
            Part::Text(_) => true,
            Part::Setting(n) if n == "sitelink" => true,
            Part::Setting(n) => n != "cookie" && n != "useragent" && declared_text(n),
        });
        if !valid {
            return Err(unsupported());
        }
        declared.push(DeclaredHeader {
            name: name.to_owned(),
            parts,
        });
    }
    Ok((agent, declared))
}
/// Render declared headers with the configured site link (upstream `SiteLink`, the
/// origin with its trailing slash) and setting values. A referenced setting must be
/// non-empty visible ASCII; values never appear in errors.
pub(super) fn render_headers(
    declared: &[DeclaredHeader],
    sitelink: &str,
    settings: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<(String, String)>> {
    declared
        .iter()
        .map(|header| {
            let mut value = String::new();
            for part in &header.parts {
                match part {
                    Part::Text(text) => value.push_str(text),
                    Part::Setting(name) if name == "sitelink" => value.push_str(sitelink),
                    Part::Setting(name) => {
                        let setting = settings
                            .get(name)
                            .filter(|v| !v.trim().is_empty() && visible_ascii(v))
                            .ok_or_else(|| {
                                anyhow!("This preset requires a valid {name} setting")
                            })?;
                        value.push_str(setting);
                    }
                }
            }
            if value.len() > 8192 || !value.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
                bail!("Invalid session header value");
            }
            Ok((header.name.clone(), value))
        })
        .collect()
}

/// One RFC6265 cookie-pair name and value, without attributes.
pub(crate) fn cookie_pair_valid(name: &str, value: &str) -> bool {
    !name.is_empty()
        && !value.is_empty()
        && value != "__jackett_expired__"
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        && value.bytes().all(|b| {
            b == 0x21
                || (0x23..=0x2b).contains(&b)
                || (0x2d..=0x3a).contains(&b)
                || (0x3c..=0x5b).contains(&b)
                || (0x5d..=0x7e).contains(&b)
        })
}
/// Admit ordinary RFC6265 cookie-pairs only, not attributes or arbitrary header text.
pub(crate) fn cookie_pairs(cookie: &str) -> Result<Vec<(&str, &str)>> {
    let mut names = BTreeSet::new();
    if cookie.is_empty()
        || cookie.len() > 4096
        || cookie
            .bytes()
            .any(|byte| !byte.is_ascii() || byte.is_ascii_control())
    {
        bail!("Invalid cookie setting");
    }
    cookie
        .split(';')
        .map(|pair| {
            pair.trim()
                .split_once('=')
                .filter(|(name, value)| names.insert(*name) && cookie_pair_valid(name, value))
                .ok_or_else(|| anyhow!("Invalid cookie setting"))
        })
        .collect()
}

struct SessionState {
    gate: Instant,
    cookies: super::cookies::Cookies,
    file: Option<super::session_store::CookieFile>,
    loaded: bool,
    dirty: bool,
    /// The cookies passed a full validation (login and TestLogin, or the stored or
    /// supplied session's test) and no later search has failed. In memory only, so
    /// a restart or a changed identity validates once more (docs/session-validation.md).
    validated: bool,
}
/// A config store's session namespace. Unchanged source identities share their
/// writer across runtime replacements; changed credentials or origins never do.
pub(crate) struct SessionRepository {
    directory: PathBuf,
    states: std::sync::Mutex<HashMap<String, Weak<Mutex<SessionState>>>>,
}
impl SessionRepository {
    pub fn new(config: &Path) -> Self {
        let mut directory = config.as_os_str().to_owned();
        directory.push(".sessions");
        Self {
            directory: directory.into(),
            states: Default::default(),
        }
    }
}
/// One configured source and origin, with its own non-redirecting, non-retrying client.
/// Engine sessions retain response cookies across edits with the same source identity
/// and across restarts. They never modify the operator's saved seed cookie.
pub struct CookieSession {
    plan: CookiePlan,
    base: url::Url,
    client: reqwest::Client,
    state: Arc<Mutex<SessionState>>,
    seed_identity: [u8; 32],
    delay: Duration,
    timeout: Duration,
    form: Option<Arc<super::form_auth::ConfiguredForm>>,
}
struct SessionRequest<'a> {
    path: &'a str,
    redirects: usize,
    limit: usize,
    fields: Option<&'a [(String, String)]>,
    /// A search POST body already encoded by the request plan (upstream encoding).
    body: Option<&'a str>,
    referer: Option<&'a str>,
    /// Pinned `BaseIndexer.Download` sends the link itself as Referer on its first
    /// request (`BaseIndexer.cs:537-548`); it wins over a declared Referer.
    self_referer: bool,
    probe: bool,
}
struct Received {
    bytes: Vec<u8>,
    content_type: String,
    url: url::Url,
    unauthorized: bool,
    /// The probe ended on a redirect rather than HTTP 401.
    redirected: bool,
    state: OwnedMutexGuard<SessionState>,
}
#[derive(Debug)]
struct MissingLogin;
impl std::fmt::Display for MissingLogin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cookie authentication rejected: expected login selector missing")
    }
}
impl std::error::Error for MissingLogin {}
impl CookieSession {
    pub fn new(
        plan: CookiePlan,
        base: &str,
        cookie: &str,
        delay_ms: u64,
        timeout_seconds: u64,
    ) -> Result<Self> {
        Self::create(plan, base, cookie, delay_ms, timeout_seconds, None)
    }
    /// Password-form sessions share cookie scoping, durability and request gates.
    /// Constructing one performs no network request and admits no catalog ID.
    pub fn new_form(
        plan: super::form_auth::FormPlan,
        base: &str,
        settings: &std::collections::BTreeMap<String, serde_json::Value>,
        delay_ms: u64,
        timeout_seconds: u64,
    ) -> Result<Self> {
        let form = Arc::new(plan.configure(settings, base)?);
        let plan = CookiePlan {
            test: form.plan.test.clone(),
            aidoru_fragment: false,
            user_agent: None,
            declared: form.plan.headers.clone(),
            header_settings: referenced_settings(&form.plan.headers)
                .into_iter()
                .filter_map(|name| {
                    let value = settings.get(&name)?.as_str()?.to_owned();
                    Some((name, value))
                })
                .collect(),
            headers: Vec::new(),
            charset: form.plan.charset,
            certificates: form.plan.certificates.clone(),
        };
        let session = Self::create(plan, base, "", delay_ms, timeout_seconds, Some(form))?;
        session.target(&session.form.as_ref().unwrap().plan.path)?;
        Ok(session)
    }
    fn create(
        mut plan: CookiePlan,
        base: &str,
        cookie: &str,
        delay_ms: u64,
        timeout_seconds: u64,
        form: Option<Arc<super::form_auth::ConfiguredForm>>,
    ) -> Result<Self> {
        let base = url::Url::parse(base).map_err(|_| anyhow!("Invalid cookie source origin"))?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
            || delay_ms > 60000
            || !(1..=120).contains(&timeout_seconds)
        {
            bail!("Invalid cookie session bounds or origin");
        }
        if plan.user_agent.as_ref().is_some_and(|s| s.is_empty()) {
            bail!("This preset requires a User-Agent setting");
        }
        plan.headers = render_headers(&plan.declared, base.as_str(), &plan.header_settings)?;
        plan.header_settings.clear();
        let (cookies, mut seed_identity) = if let Some(form) = &form {
            let mut identity = Sha256::new();
            if form.plan.direct_post {
                identity.update(b"Rackett credential POST v1");
                identity.update((form.plan.path.len() as u64).to_be_bytes());
                identity.update(form.plan.path.as_bytes());
            } else {
                identity.update(b"Rackett form credentials v1");
            }
            identity.update(serde_json::to_vec(&form.inputs)?);
            // A declared login cookie seed starts the form session and is part of
            // its identity, so a changed seed never reuses an older session.
            // Seedless forms keep their existing identity (and stored sessions).
            let seed = form.plan.cookies.as_deref().unwrap_or("");
            if !seed.is_empty() {
                identity.update(b"login cookies");
                identity.update((seed.len() as u64).to_be_bytes());
                identity.update(seed.as_bytes());
            }
            (
                if seed.is_empty() {
                    super::cookies::Cookies::empty(&base)
                } else {
                    super::cookies::Cookies::new(&base, seed)?
                },
                identity.finalize().into(),
            )
        } else {
            (
                super::cookies::Cookies::new(&base, cookie)?,
                Sha256::digest(cookie.as_bytes()).into(),
            )
        };
        if let Some(user_agent) = &plan.user_agent {
            // Browser-bound cookies must not survive a changed User-Agent identity.
            let mut digest = Sha256::new();
            digest.update(b"Rackett cookie User-Agent identity v1");
            digest.update(seed_identity);
            digest.update(user_agent.as_bytes());
            seed_identity = digest.finalize().into();
        }
        if !plan.headers.is_empty() {
            // Header-bound values (a CSRF token, a site-link Referer) are part of the
            // session identity, like the User-Agent: a changed value starts again from
            // the seed, as pinned ApplyConfiguration does. Sources without declared
            // headers keep their existing identity and stored sessions.
            let mut digest = Sha256::new();
            digest.update(b"Rackett session headers v1");
            digest.update(seed_identity);
            for (name, value) in &plan.headers {
                for part in [name.to_ascii_lowercase(), value.clone()] {
                    digest.update((part.len() as u64).to_be_bytes());
                    digest.update(part.as_bytes());
                }
            }
            seed_identity = digest.finalize().into();
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .user_agent(plan.user_agent.as_deref().unwrap_or("Rackett/0.1"));
        let client = crate::pinned_tls::apply(client, plan.certificates.as_ref(), base.as_str())?
            .build()
            .map_err(|_| anyhow!("Cookie client creation failed"))?;
        let session = Self {
            plan,
            base,
            client,
            state: Arc::new(Mutex::new(SessionState {
                gate: Instant::now(),
                cookies,
                file: None,
                loaded: false,
                dirty: false,
                validated: false,
            })),
            seed_identity,
            delay: Duration::from_millis(delay_ms),
            timeout: Duration::from_secs(timeout_seconds),
            form,
        };
        if let Some(test) = &session.plan.test {
            session.target(&test.path)?;
        }
        Ok(session)
    }
    /// The request plan's rendered search headers must be exactly the session's own
    /// set (the bound User-Agent and the rendered declared headers); the session then
    /// sends its set. A mismatch is rejected without naming any value.
    pub(crate) fn check_search_headers(&self, headers: &[(String, String)]) -> Result<()> {
        let normalize = |pairs: &mut dyn Iterator<Item = (&String, &String)>| {
            let mut pairs: Vec<_> = pairs
                .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
                .collect();
            pairs.sort();
            pairs
        };
        let agent = self
            .plan
            .user_agent
            .as_ref()
            .map(|agent| ("user-agent".to_owned(), agent.clone()));
        let expected = normalize(
            &mut agent
                .iter()
                .map(|(k, v)| (k, v))
                .chain(self.plan.headers.iter().map(|(k, v)| (k, v))),
        );
        if normalize(&mut headers.iter().map(|(k, v)| (k, v))) != expected {
            bail!("Unsupported authenticated search headers");
        }
        Ok(())
    }
    /// The `Cookie` request header: pinned HttpWebClient2 puts the stored session in
    /// its CookieContainer and the definition's `Cookie` among the request headers
    /// (`HttpWebClient2.cs:113-135`). .NET 9 SocketsHttpHandler then writes the custom
    /// value first and appends the container's cookies after `"; "`, or sends the
    /// container's alone (`HttpConnection.cs:363-378`, `:443-448`, `:465-470`).
    fn cookie_header(&self, declared: bool, jar: Option<String>) -> Option<String> {
        let literal = declared
            .then(|| {
                self.plan
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("cookie"))
                    .map(|(_, v)| v.as_str())
            })
            .flatten();
        match (literal, jar) {
            (Some(literal), Some(jar)) => Some(format!("{literal}; {jar}")),
            (Some(literal), None) => Some(literal.to_owned()),
            (None, jar) => jar,
        }
    }
    pub(crate) fn bind(
        &mut self,
        repository: &SessionRepository,
        source: &str,
        builtin: &str,
    ) -> Result<()> {
        let mut digest = Sha256::new();
        digest.update(b"Rackett cookie session v1");
        for part in [
            source.as_bytes(),
            builtin.as_bytes(),
            self.base.as_str().as_bytes(),
            &self.seed_identity,
        ] {
            digest.update((part.len() as u64).to_be_bytes());
            digest.update(part);
        }
        let fingerprint = format!("{:x}", digest.finalize());
        let mut states = repository
            .states
            .lock()
            .map_err(|_| anyhow!("Cookie session registry unavailable"))?;
        states.retain(|_, state| state.strong_count() > 0);
        if let Some(state) = states.get(&fingerprint).and_then(Weak::upgrade) {
            self.state = state;
        } else {
            let state = Arc::get_mut(&mut self.state)
                .ok_or_else(|| anyhow!("Cookie session already shared"))?
                .get_mut();
            state.file = Some(super::session_store::CookieFile::new(
                &repository.directory,
                fingerprint.clone(),
            ));
            states.insert(fingerprint, Arc::downgrade(&self.state));
        }
        Ok(())
    }
    /// The blocking worker owns the writer until load/save finishes, even after
    /// its caller times out. A failed save leaves the in-memory update dirty;
    /// the next request must persist it before sending any credentials.
    async fn durable(
        &self,
        mut state: OwnedMutexGuard<SessionState>,
    ) -> Result<OwnedMutexGuard<SessionState>> {
        let Some(file) = state.file.clone() else {
            return Ok(state);
        };
        if state.loaded && !state.dirty {
            return Ok(state);
        }
        let base = self.base.clone();
        tokio::task::spawn_blocking(move || {
            if !state.loaded {
                if let Some(cookies) = file.load(&base)? {
                    state.cookies = cookies;
                }
                state.loaded = true;
            }
            if state.dirty {
                file.save(&state.cookies)?;
                state.dirty = false;
            }
            Ok(state)
        })
        .await
        .map_err(|_| anyhow!("Cookie session storage worker failed"))?
    }
    fn target(&self, path: &str) -> Result<url::Url> {
        let target = self
            .base
            .join(path)
            .map_err(|_| anyhow!("Invalid cookie request target"))?;
        super::checked_url(target.as_str(), &self.base)?;
        if target.fragment().is_some() {
            bail!("Cookie request fragment rejected");
        }
        Ok(target)
    }
    async fn receive(
        &self,
        path: &str,
        redirects: usize,
        limit: usize,
        self_referer: bool,
        state: OwnedMutexGuard<SessionState>,
    ) -> Result<(Vec<u8>, String, OwnedMutexGuard<SessionState>)> {
        let response = self
            .receive_request(
                SessionRequest {
                    path,
                    redirects,
                    limit,
                    fields: None,
                    body: None,
                    referer: None,
                    self_referer,
                    probe: false,
                },
                state,
            )
            .await?;
        Ok((response.bytes, response.content_type, response.state))
    }
    async fn receive_request(
        &self,
        request: SessionRequest<'_>,
        state: OwnedMutexGuard<SessionState>,
    ) -> Result<Received> {
        let mut state = self.durable(state).await?;
        let mut target = self.target(request.path)?;
        let mut fields = request.fields;
        let mut body = request.body;
        for n in 0..=request.redirects {
            tokio::time::sleep_until(state.gate.into()).await;
            state.gate = Instant::now() + self.delay;
            let mut outgoing = if let Some(fields) = fields {
                if self.plan.charset == Charset::Utf8 {
                    self.client.post(target.clone()).form(fields)
                } else {
                    // Pinned WebClient.FormUrlEncodedContentWithEncoding.
                    let body = self
                        .plan
                        .charset
                        .form_body(fields)
                        .map_err(|e| anyhow!(e))?;
                    self.client
                        .post(target.clone())
                        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                        .body(body)
                }
            } else if let Some(body) = body {
                self.client
                    .post(target.clone())
                    .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(body.to_owned())
            } else {
                self.client.get(target.clone())
            };
            // Declared headers go with the first request only: pinned FollowIfRedirect
            // builds each hop without custom headers (`BaseIndexer.cs:745-770`).
            let declared = n == 0;
            let referer = if declared && request.self_referer {
                Some(target.as_str().to_owned())
            } else {
                request.referer.map(str::to_owned)
            };
            if declared {
                for (name, value) in &self.plan.headers {
                    let name = name.to_ascii_lowercase();
                    // A request Referer replaces a declared one: pinned HttpWebClient2
                    // sets `Headers.Referrer` after the custom headers (`HttpWebClient2.cs:146-147`).
                    if name == "cookie" || (name == "referer" && referer.is_some()) {
                        continue;
                    }
                    outgoing = outgoing.header(name, value.as_str());
                }
            }
            if let Some(referer) = referer {
                outgoing = outgoing.header(reqwest::header::REFERER, referer);
            }
            let jar = state
                .cookies
                .header(&target, chrono::Utc::now().timestamp())?
                .map(|h| h.to_str().map(str::to_owned))
                .transpose()
                .map_err(|_| anyhow!("Invalid session cookie header"))?;
            if let Some(cookie) = self.cookie_header(declared, jar) {
                outgoing = outgoing.header(COOKIE, cookie);
            }
            let response = outgoing
                .send()
                .await
                .map_err(|_| anyhow!("Cookie source transport failed"))?;
            state.dirty |= state.cookies.update(
                &target,
                response.headers(),
                chrono::Utc::now().timestamp(),
            )?;
            state = self.durable(state).await?;
            if response.headers().contains_key("refresh") {
                bail!("Cookie source refresh redirect rejected");
            }
            // A challenge can return a convincing login page with HTTP 200. Reject
            // its explicit marker before status, HTML, search, or download handling.
            if response
                .headers()
                .get("cf-mitigated")
                .is_some_and(|value| value == "challenge")
            {
                bail!("Cookie source Cloudflare challenge detected; challenge solving is not supported");
            }
            if response.status().is_redirection() {
                // Pinned CheckIfLoginIsNeeded treats a same-origin search
                // redirect as expiry. Do not follow it or send credentials to
                // its target: authenticate through the declared login plan,
                // then retry the original search once.
                if request.probe && request.redirects == 0 {
                    if ![301, 302, 303, 307, 308].contains(&response.status().as_u16()) {
                        bail!("Cookie source redirect rejected");
                    }
                    let location = response
                        .headers()
                        .get(LOCATION)
                        .and_then(|h| h.to_str().ok())
                        .ok_or_else(|| anyhow!("Invalid cookie redirect"))?;
                    self.target(
                        target
                            .join(location)
                            .map_err(|_| anyhow!("Invalid cookie redirect"))?
                            .as_str(),
                    )?;
                    return Ok(Received {
                        bytes: Vec::new(),
                        content_type: String::new(),
                        url: target,
                        unauthorized: true,
                        redirected: true,
                        state,
                    });
                }
                if n == request.redirects
                    || ![301, 302, 303, 307, 308].contains(&response.status().as_u16())
                {
                    bail!("Cookie source redirect rejected");
                }
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|h| h.to_str().ok())
                    .ok_or_else(|| anyhow!("Invalid cookie redirect"))?;
                target = self.target(
                    target
                        .join(location)
                        .map_err(|_| anyhow!("Invalid cookie redirect"))?
                        .as_str(),
                )?;
                if [301, 302, 303].contains(&response.status().as_u16()) {
                    fields = None;
                    body = None;
                }
                continue;
            }
            if request.probe && response.status() == reqwest::StatusCode::UNAUTHORIZED {
                return Ok(Received {
                    bytes: Vec::new(),
                    content_type: String::new(),
                    url: target,
                    unauthorized: true,
                    redirected: false,
                    state,
                });
            }
            if !response.status().is_success() {
                bail!("Cookie source returned HTTP error");
            }
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_owned();
            if response
                .content_length()
                .is_some_and(|n| n > request.limit as u64)
            {
                bail!("Cookie response exceeds byte budget");
            }
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| anyhow!("Unable to read cookie response"))?;
                if bytes.len() + chunk.len() > request.limit {
                    bail!("Cookie response exceeds byte budget");
                }
                bytes.extend_from_slice(&chunk);
            }
            state.gate = Instant::now() + self.delay;
            return Ok(Received {
                bytes,
                content_type,
                url: target,
                unauthorized: false,
                redirected: false,
                state,
            });
        }
        unreachable!()
    }
    fn decode_html(bytes: Vec<u8>, content_type: &str, charset: Charset) -> Result<String> {
        let mut parts = content_type.split(';');
        if !matches!(
            parts
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "" | "text/html"
        ) || (charset == Charset::Utf8
            && parts.any(|p| {
                let p = p.trim().to_ascii_lowercase();
                p.starts_with("charset=")
                    && !["charset=utf-8", "charset=\"utf-8\""].contains(&p.as_str())
            }))
        {
            bail!("Cookie validation requires UTF-8 HTML");
        }
        // WebResult.Encoding: a non-UTF-8 definition encoding wins over the header charset.
        let body = charset
            .decode(bytes)
            .map_err(|_| anyhow!("Cookie validation requires UTF-8 HTML"))?;
        if super::flaresolverr::challenge_markers(body.as_bytes()) {
            bail!(
                "Cookie source Cloudflare challenge detected; challenge solving is not supported"
            );
        }
        Ok(body)
    }
    async fn parser_permit() -> Result<OwnedSemaphorePermit> {
        static PARSERS: OnceLock<Arc<Semaphore>> = OnceLock::new();
        Ok(PARSERS
            .get_or_init(|| Arc::new(Semaphore::new(8)))
            .clone()
            .acquire_owned()
            .await?)
    }
    /// The declared positive login marker, if any.
    fn marker(&self) -> Option<&str> {
        self.plan.test.as_ref().and_then(|t| t.selector.as_deref())
    }
    async fn html(&self, bytes: Vec<u8>, content_type: &str, search: bool) -> Result<String> {
        let body = Self::decode_html(bytes, content_type, self.plan.charset)?;
        let permit = Self::parser_permit().await?;
        let selector = if search && self.plan.aidoru_fragment {
            "table.ttable_headinner".into()
        } else if let Some(selector) = self.marker() {
            selector.to_owned()
        } else {
            // Pinned TestLogin and CheckIfLoginIsNeeded inspect no page without a
            // test selector (`CardigannIndexer.cs:935`, `:977-984`).
            return Ok(body);
        };
        tokio::task::spawn_blocking(move || {
            // Retain the permit even if the caller's deadline cancels the join.
            let _permit = permit;
            if !crate::cardigann_exec::query_selector_exists(&body, &selector)
                .map_err(|_| anyhow!("Cookie authentication document is unsupported"))?
            {
                return Err(MissingLogin.into());
            }
            Ok(body)
        })
        .await
        .map_err(|_| anyhow!("Cookie response parser failed"))?
    }

    /// A search response. Pinned `CheckIfLoginIsNeeded` (`CardigannIndexer.cs:951-997`)
    /// applies the login test selector only when Content-Type contains `text/html` or
    /// is absent; JSON and XML search responses are otherwise returned as text.
    /// `typed` is a definition-declared JSON or XML response; HTML searches keep the
    /// strict HTML check and login selector.
    async fn search_page(&self, bytes: Vec<u8>, content_type: &str, typed: bool) -> Result<String> {
        if !typed || content_type.is_empty() || content_type.contains("text/html") {
            return self.html(bytes, content_type, true).await;
        }
        if super::flaresolverr::challenge_markers(&bytes) {
            bail!(
                "Cookie source Cloudflare challenge detected; challenge solving is not supported"
            );
        }
        self.plan
            .charset
            .decode(bytes)
            .map_err(|_| anyhow!("Cookie search response is not valid text"))
    }
    /// Always checks the session; only success marks it validated. A failure or a
    /// cancelled deadline drops the guard with the flag cleared.
    async fn validate(
        &self,
        mut state: OwnedMutexGuard<SessionState>,
    ) -> Result<OwnedMutexGuard<SessionState>> {
        state.validated = false;
        let mut state = if self.form.is_some() {
            self.validate_form(state, false).await?
        } else if let Some(test) = &self.plan.test {
            let (bytes, ct, state) = self
                .receive(&test.path, 1, 8 * 1024 * 1024, false, state)
                .await?;
            self.html(bytes, &ct, false).await?;
            state
        } else {
            // Without `login.test`, pinned TestLogin returns before any request
            // (`CardigannIndexer.cs:896-899`).
            state
        };
        state.validated = true;
        Ok(state)
    }
    async fn validate_form(
        &self,
        state: OwnedMutexGuard<SessionState>,
        force_login: bool,
    ) -> Result<OwnedMutexGuard<SessionState>> {
        let form = self
            .form
            .clone()
            .ok_or_else(|| anyhow!("Missing form plan"))?;
        let mut state = self.durable(state).await?;
        let test = self.plan.test.as_ref();
        let target = self.target(test.map_or("", |t| &t.path))?;
        let now = chrono::Utc::now().timestamp();
        let stored = state.cookies.header(&target, now)?;
        // A jar holding only the declared `login.cookies` seed is not a session: pinned
        // DoLogin sets the seed as the cookie header and logs in straight away
        // (`CardigannIndexer.cs:590-591`, `:1026-1027`), with no login test first.
        let seed_only = match form.plan.cookies.as_deref().filter(|s| !s.is_empty()) {
            Some(seed) => {
                stored.is_some()
                    && stored
                        == super::cookies::Cookies::new(&self.base, seed)?.header(&target, now)?
            }
            None => false,
        };
        if !force_login && stored.is_some() && !seed_only {
            // Without a test, a stored session is trusted until a search redirect
            // (pinned CheckIfLoginIsNeeded, `CardigannIndexer.cs:953-980`).
            let Some(test) = test else {
                return Ok(state);
            };
            let reply = self
                .receive_request(
                    SessionRequest {
                        path: &test.path,
                        redirects: 1,
                        limit: 8 * 1024 * 1024,
                        fields: None,
                        body: None,
                        referer: None,
                        self_referer: false,
                        probe: true,
                    },
                    state,
                )
                .await?;
            state = reply.state;
            if !reply.unauthorized {
                match self.html(reply.bytes, &reply.content_type, false).await {
                    Ok(_) => return Ok(state),
                    Err(error) if error.is::<MissingLogin>() => {}
                    Err(error) => return Err(error),
                }
            }
        }
        let (submission, landing_url, state) = if form.plan.direct_post {
            // Pinned Cardigann POST login clears its old cookie seed before sending
            // credentials. Persist that reset before network, including after expiry.
            state.cookies = super::cookies::Cookies::empty(&self.base);
            state.dirty = true;
            let state = self.durable(state).await?;
            let target = self.target(&form.plan.path)?;
            (
                super::form_auth::Submission {
                    target: target.to_string(),
                    fields: form.inputs.clone(),
                },
                self.base.clone(),
                state,
            )
        } else {
            let landing = self
                .receive_request(
                    SessionRequest {
                        path: &form.plan.path,
                        redirects: 0,
                        limit: 8 * 1024 * 1024,
                        fields: None,
                        body: None,
                        referer: Some(self.base.as_str()),
                        self_referer: false,
                        probe: false,
                    },
                    state,
                )
                .await?;
            let body = Self::decode_html(landing.bytes, &landing.content_type, self.plan.charset)?;
            let landing_url = landing.url;
            let permit = Self::parser_permit().await?;
            let prepared_form = form.clone();
            let prepared_url = landing_url.clone();
            let submission = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                prepared_form.prepare(&body, &prepared_url)
            })
            .await
            .map_err(|_| anyhow!("Login form parser failed"))??;
            (submission, landing_url, landing.state)
        };
        // Both action forms are checked again before credentials or cookies are attached.
        let posted = self
            .receive_request(
                SessionRequest {
                    path: &submission.target,
                    redirects: 5,
                    limit: 8 * 1024 * 1024,
                    fields: Some(&submission.fields),
                    body: None,
                    referer: Some(landing_url.as_str()),
                    self_referer: false,
                    probe: false,
                },
                state,
            )
            .await?;
        let body = Self::decode_html(posted.bytes, &posted.content_type, self.plan.charset)?;
        let permit = Self::parser_permit().await?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            form.check_response(&body)
        })
        .await
        .map_err(|_| anyhow!("Login response parser failed"))??;
        // Pinned ApplyConfiguration and Relogin run TestLogin, which sends nothing
        // without a test (`CardigannIndexer.cs:885`, `:896-899`, `:1107`).
        let Some(test) = test else {
            return Ok(posted.state);
        };
        let (bytes, ct, state) = self
            .receive(&test.path, 1, 8 * 1024 * 1024, false, posted.state)
            .await?;
        self.html(bytes, &ct, false).await?;
        Ok(state)
    }
    pub async fn test(&self) -> Result<()> {
        tokio::time::timeout(self.timeout, async {
            self.validate(self.state.clone().lock_owned().await)
                .await
                .map(|_| ())
        })
        .await
        .map_err(|_| anyhow!("Cookie authentication deadline exceeded"))?
    }
    /// Supplied-cookie searches never retry. Form sessions may authenticate and
    /// retry once when the search redirects, or loses its positive login marker or
    /// returns 401 when the definition declares one.
    pub async fn fetch_html(&self, path: &str) -> Result<String> {
        self.fetch_search(path, None, false).await
    }
    /// A search page through the session; `body` is a plan-encoded POST search and
    /// `typed` a definition-declared JSON or XML response.
    ///
    /// Pinned PerformQuery sends no test request; each search response itself is
    /// checked and a failed check relogs once (`CardigannIndexer.cs:1616-1632`,
    /// `:1798-1813`). Only an unvalidated session (first use, or after a failed
    /// search) is tested first. Each search consumes the trust and only an accepted
    /// page restores it, so errors and cancelled deadlines leave it cleared.
    pub async fn fetch_search(
        &self,
        path: &str,
        body: Option<&str>,
        typed: bool,
    ) -> Result<String> {
        let target = self.target(path)?;
        if self.plan.aidoru_fragment && target.path() != self.target("get_ttable.php")?.path() {
            bail!("Unsupported AJAX cookie search target");
        }
        tokio::time::timeout(self.timeout, async {
            let gate = self.state.clone().lock_owned().await;
            let mut gate = if gate.validated {
                gate
            } else {
                self.validate(gate).await?
            };
            if self.form.is_some() {
                for attempt in 0..=1 {
                    gate.validated = false;
                    let reply = self
                        .receive_request(
                            SessionRequest {
                                path,
                                redirects: 0,
                                limit: 8 * 1024 * 1024,
                                fields: None,
                                body,
                                referer: None,
                                self_referer: false,
                                probe: true,
                            },
                            gate,
                        )
                        .await?;
                    // Pinned CheckIfLoginIsNeeded: a redirect always means relogin; without
                    // a test selector nothing else does (`CardigannIndexer.cs:953-984`).
                    let result =
                        if reply.unauthorized && (reply.redirected || self.marker().is_some()) {
                            Err(anyhow!(MissingLogin))
                        } else if reply.unauthorized {
                            Err(anyhow!("Cookie source returned HTTP error"))
                        } else {
                            self.search_page(reply.bytes, &reply.content_type, typed)
                                .await
                        };
                    match result {
                        Ok(body) => {
                            let mut state = reply.state;
                            state.validated = true;
                            return Ok(body);
                        }
                        Err(error) if attempt == 0 && error.is::<MissingLogin>() => {
                            gate = self.validate_form(reply.state, true).await?;
                        }
                        Err(error) => return Err(error),
                    }
                }
                unreachable!()
            }
            // Supplied cookies cannot reauthenticate: an expired session fails this
            // search, and the next one tests the session again.
            gate.validated = false;
            let response = self
                .receive_request(
                    SessionRequest {
                        path,
                        redirects: 0,
                        limit: 8 * 1024 * 1024,
                        fields: None,
                        body,
                        referer: None,
                        self_referer: false,
                        probe: false,
                    },
                    gate,
                )
                .await?;
            let page = self
                .search_page(response.bytes, &response.content_type, typed)
                .await?;
            let mut state = response.state;
            state.validated = true;
            Ok(page)
        })
        .await
        .map_err(|_| anyhow!("Cookie search deadline exceeded"))?
    }
    /// A `download.before` request through the session, with the release link as
    /// Referer. Upstream ignores its response; same-origin redirects are followed.
    pub async fn send_before(
        &self,
        path: &str,
        fields: Option<&[(String, String)]>,
        referer: &str,
    ) -> Result<()> {
        self.target(path)?;
        tokio::time::timeout(self.timeout, async {
            let gate = self.validate(self.state.clone().lock_owned().await).await?;
            self.receive_request(
                SessionRequest {
                    path,
                    redirects: 5,
                    limit: 8 * 1024 * 1024,
                    fields,
                    body: None,
                    referer: Some(referer),
                    self_referer: false,
                    probe: false,
                },
                gate,
            )
            .await?;
            Ok(())
        })
        .await
        .map_err(|_| anyhow!("Cookie download deadline exceeded"))?
    }
    /// Auth remains in the source session; callers expose only their opaque download ticket.
    /// Torrent validation belongs to the caller. No download resolver is implied.
    pub async fn fetch_download(&self, path: &str) -> Result<Vec<u8>> {
        self.target(path)?;
        tokio::time::timeout(self.timeout, async {
            let gate = self.validate(self.state.clone().lock_owned().await).await?;
            let (bytes, _, _gate) = self.receive(path, 5, 32 * 1024 * 1024, true, gate).await?;
            Ok(bytes)
        })
        .await
        .map_err(|_| anyhow!("Cookie download deadline exceeded"))?
    }
}
