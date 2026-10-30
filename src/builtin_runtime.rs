//! Bounded transport around the immutable offline interpreter.
pub mod api_auth;
pub mod auth;
mod cookies;
pub mod download;
pub mod flaresolverr;
pub mod form_auth;
pub mod get_login;
mod session_store;
use crate::{
    builtin::{DetailMagnet, Runtime},
    cardigann_exec::request::{BuiltRequest, Charset},
    models::*,
};
use anyhow::{anyhow, bail, Result};
use futures_util::StreamExt;
use std::{
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;
fn checked_url(raw: &str, base: &url::Url) -> Result<url::Url> {
    let u = url::Url::parse(raw).map_err(|_| anyhow!("Invalid plan target"))?;
    if !matches!(u.scheme(), "http" | "https")
        || !u.username().is_empty()
        || u.password().is_some()
        || u.origin() != base.origin()
    {
        bail!("Plan target must remain on configured origin");
    }
    Ok(u)
}
fn headers(r: &BuiltRequest) -> Result<reqwest::header::HeaderMap> {
    let mut headers = reqwest::header::HeaderMap::new();
    for (k, v) in &r.headers {
        if v.chars().any(char::is_control)
            || [
                "host",
                "connection",
                "transfer-encoding",
                "content-length",
                "upgrade",
                "keep-alive",
                "proxy-authenticate",
                "proxy-connection",
                "proxy-authorization",
                "te",
                "trailer",
            ]
            .contains(&k.to_ascii_lowercase().as_str())
        {
            bail!("Forbidden plan header");
        }
        headers.append(
            reqwest::header::HeaderName::from_bytes(k.as_bytes())
                .map_err(|_| anyhow!("Invalid plan header"))?,
            v.parse()
                .map_err(|_| anyhow!("Invalid plan header value"))?,
        );
    }
    Ok(headers)
}
enum Sent {
    Response(reqwest::Response, bool),
    /// A recognized challenge on this same-origin target, after `bool` redirects.
    Challenge(url::Url, bool),
}
async fn send(
    client: &reqwest::Client,
    request: &BuiltRequest,
    base: &url::Url,
    gate: &mut Instant,
    delay_ms: u64,
    headers: reqwest::header::HeaderMap,
    solving: bool,
) -> Result<Sent> {
    let mut target = checked_url(&request.url, base)?;
    // Counted per followed hop: a redirect back to the original URL is still a redirect.
    for redirects in 0..=5 {
        tokio::time::sleep_until((*gate).into()).await;
        *gate = Instant::now() + Duration::from_millis(delay_ms);
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|_| anyhow!("Invalid method"))?;
        let mut req = client
            .request(method, target.clone())
            .headers(headers.clone());
        if let Some(body) = &request.body {
            req = req
                .header(
                    reqwest::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(body.clone());
        }
        let response = req
            .send()
            .await
            .map_err(|_| anyhow!("Bundled source transport failed (connection or timeout)"))?;
        let status = response.status();
        if status.is_redirection() {
            if redirects == 5 {
                bail!("Bundled source redirect limit exceeded");
            }
            if ![301, 302, 303, 307, 308].contains(&status.as_u16()) {
                bail!("Unsupported source redirect");
            }
            if request.method != "GET" && ![307, 308].contains(&status.as_u16()) {
                bail!("Unsafe method-changing redirect rejected");
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| anyhow!("Invalid source redirect"))?;
            target = checked_url(
                target
                    .join(location)
                    .map_err(|_| anyhow!("Invalid source redirect"))?
                    .as_str(),
                base,
            )?;
            continue;
        }
        // Report only locally defined diagnostics. Upstream bodies, URLs and arbitrary
        // headers can contain credentials and must not reach management/protocol errors.
        if response
            .headers()
            .get("cf-mitigated")
            .is_some_and(|value| value == "challenge")
        {
            if solving {
                return Ok(Sent::Challenge(target, redirects > 0));
            }
            bail!("Bundled source returned HTTP {status}: Cloudflare challenge detected; challenge solving is not supported");
        }
        if status.is_client_error() || status.is_server_error() {
            // Only with a solver configured is a 403/503 body read, bounded, for markers.
            if solving
                && [403, 503].contains(&status.as_u16())
                && read_body(response)
                    .await
                    .is_ok_and(|body| flaresolverr::challenge_markers(&body))
            {
                return Ok(Sent::Challenge(target, redirects > 0));
            }
            let advice = match status.as_u16() {
                401 => "Authentication required; check the source's API key or credentials",
                403 => {
                    "Access denied by the source; check account permissions or access restrictions"
                }
                404 => "Search endpoint not found; check the source URL or preset",
                429 => "Rate limit exceeded; wait before searching again",
                500..=599 => "The source is unavailable or failing; try again later",
                _ => "The source rejected the search request",
            };
            bail!("Bundled source returned HTTP {status}: {advice}");
        }
        return Ok(Sent::Response(response, redirects > 0));
    }
    unreachable!("Bounded redirect loop returns")
}
/// Bounds concurrent blocking HTML/JSON parsing across every source.
async fn parser_permit() -> Result<tokio::sync::OwnedSemaphorePermit> {
    static PARSERS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    Ok(PARSERS
        .get_or_init(|| Arc::new(Semaphore::new(8)))
        .clone()
        .acquire_owned()
        .await?)
}
/// A magnet URI naming a BitTorrent v1 infohash, in hex or base32.
fn btih_magnet(url: &url::Url) -> bool {
    url.scheme() == "magnet"
        && url.query_pairs().any(|(k, v)| {
            k == "xt"
                && v.strip_prefix("urn:btih:").is_some_and(|h| {
                    (h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit()))
                        || (h.len() == 32
                            && h.bytes()
                                .all(|b| b.is_ascii_alphabetic() || (b'2'..=b'7').contains(&b)))
                })
        })
}
/// Fixed, page-free outcomes of resolving a ticketed detail page to its magnet. Messages
/// never carry the page, its URLs or source credentials.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailFailure {
    /// The ticket target is not a same-origin detail page; nothing was sent.
    Target,
    /// Transport, status, redirect, challenge, size or encoding failure.
    Fetch,
    /// No link matched the primary magnet selector (nor a magnet fallback).
    NoMagnet,
    /// Only the iTorrents fallback matched; that cross-origin download is unsupported.
    FallbackUnsupported,
    /// The first primary link is not a plain BitTorrent magnet URI.
    InvalidMagnet,
}
impl std::fmt::Display for DetailFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Target => "Download target is not a source detail page",
            Self::Fetch => "Source detail page could not be fetched",
            Self::NoMagnet => "Source detail page has no magnet link",
            Self::FallbackUnsupported => {
                "Source detail page has no magnet link; its iTorrents fallback is unsupported"
            }
            Self::InvalidMagnet => "Source detail page magnet link is invalid",
        })
    }
}
impl std::error::Error for DetailFailure {}
/// A ticket target the detail resolver may fetch: a same-origin `/torrent/` page (the
/// only links 1337x rows produce), without credentials, query or fragment.
pub fn detail_target(base: &url::Url, raw: &str) -> Result<url::Url, DetailFailure> {
    let u = checked_url(raw, base).map_err(|_| DetailFailure::Target)?;
    if u.query().is_some() || u.fragment().is_some() || !u.path().starts_with("/torrent/") {
        return Err(DetailFailure::Target);
    }
    Ok(u)
}
/// The resolved form of an admitted magnet href: a plain `magnet:?` URI without
/// whitespace, controls or fragment that names a BitTorrent infohash.
/// Search headers to send with a `login: get` source's downloads, as upstream sends
/// `Download.Headers ?? Search.Headers`. Other sources keep their existing downloads.
pub fn get_login_download_headers(
    rt: &Runtime,
    source: &Source,
) -> Result<reqwest::header::HeaderMap> {
    if source.get_login.is_none() {
        return Ok(reqwest::header::HeaderMap::new());
    }
    let ctx = rt.plan.context(
        &rt.settings,
        &crate::models::SearchQuery::default(),
        chrono::Utc::now(),
    );
    let requests = rt
        .plan
        .build_requests(&ctx)
        .map_err(|_| anyhow!("Unable to build bundled search requests"))?;
    match requests.first() {
        Some(r) => headers(r),
        None => Ok(reqwest::header::HeaderMap::new()),
    }
}
pub(crate) fn detail_magnet_uri(raw: &str) -> Option<String> {
    if raw.len() > 16 * 1024
        || !raw.starts_with("magnet:?")
        || raw.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return None;
    }
    let url = url::Url::parse(raw).ok()?;
    (url.cannot_be_a_base()
        && url.path().is_empty()
        && url.fragment().is_none()
        && btih_magnet(&url))
    .then(|| url.into())
}
/// Upstream's download selector loop over one fetched page (`CardigannIndexer.cs:2142`):
/// the first primary match in document order wins and is never skipped for a later one.
fn select_detail_magnet(policy: DetailMagnet, page: &str) -> Result<String, DetailFailure> {
    let first = |needle: &str| {
        crate::cardigann_exec::query_selector_attribute(
            page,
            &format!("ul li a[href*=\"{needle}\"]"),
            "href",
        )
        .map_err(|_| DetailFailure::Fetch)
    };
    if let Some(href) = first("magnet:")? {
        let [from, to] = crate::builtin::ITORRENTS_REWRITE;
        return detail_magnet_uri(&href.replace(from, to)).ok_or(DetailFailure::InvalidMagnet);
    }
    match policy.fallback {
        crate::builtin::Fallback::ITorrents if first("//itorrents.")?.is_some() => {
            Err(DetailFailure::FallbackUnsupported)
        }
        _ => Err(DetailFailure::NoMagnet),
    }
}
async fn read_body(response: reqwest::Response) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|n| n > 8 * 1024 * 1024)
    {
        bail!("Response exceeds 8 MiB");
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| anyhow!("Unable to read bundled response"))?;
        if bytes.len() + chunk.len() > 8 * 1024 * 1024 {
            bail!("Response exceeds 8 MiB");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
/// Pairs of the plan's own Cookie header(s), such as EZTV's pinned layout cookie.
fn plan_cookies(request: &BuiltRequest) -> Result<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    for (_, value) in request
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("cookie"))
    {
        for (name, value) in
            auth::cookie_pairs(value).map_err(|_| anyhow!("Unable to compose source cookies"))?
        {
            pairs.push((name.to_owned(), value.to_owned()));
        }
    }
    Ok(pairs)
}

/// Epoch-scoped HTTP state of one configured bundled source: its saved cookie, the
/// optional challenge solver and any solver clearance. The engine builds one per source
/// runtime, so source edits, deletion and restart discard clearance with the epoch;
/// nothing is shared across sources. No accessor, Debug or Serialize exposes its values.
pub struct Source {
    pub(crate) api_session: Option<api_auth::ApiSession>,
    pub(crate) cookie_session: Option<auth::CookieSession>,
    saved: Vec<(String, String)>,
    solver: Option<Arc<flaresolverr::Solver>>,
    clearance: std::sync::Mutex<Option<flaresolverr::Clearance>>,
    /// `login: get` probe and its configured URL; see [`get_login`].
    get_login: Option<(get_login::GetLogin, url::Url)>,
    /// Whether this runtime's GET login probe has succeeded.
    probed: std::sync::atomic::AtomicBool,
}
impl Source {
    /// A saved cookie and the solver are admitted only for [`crate::builtin::CHALLENGE_IDS`].
    pub fn new(i: &Indexer, solver: Option<Arc<flaresolverr::Solver>>) -> Result<Self> {
        let admitted =
            crate::builtin::id(i).is_ok_and(|id| crate::builtin::CHALLENGE_IDS.contains(&id));
        if !i.cookie.is_empty() && !admitted {
            bail!("Use bundled preset settings; generic credentials are unsupported");
        }
        let saved = if i.cookie.is_empty() {
            Vec::new()
        } else {
            auth::cookie_pairs(&i.cookie)?
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect()
        };
        Ok(Self {
            api_session: crate::builtin::api_session(i)?,
            cookie_session: crate::builtin::cookie_session(i)?,
            saved,
            solver: solver.filter(|_| admitted),
            clearance: std::sync::Mutex::new(None),
            get_login: crate::builtin::get_login(i)?,
            probed: std::sync::atomic::AtomicBool::new(false),
        })
    }
    fn clearance(&self) -> Option<flaresolverr::Clearance> {
        let mut clearance = self.clearance.lock().unwrap_or_else(|e| e.into_inner());
        if clearance.as_ref().is_some_and(|c| !c.fresh()) {
            *clearance = None;
        }
        clearance.clone()
    }
    fn store(&self, clearance: Option<flaresolverr::Clearance>) {
        *self.clearance.lock().unwrap_or_else(|e| e.into_inner()) = clearance;
    }
    /// The plan's headers with one Cookie header: plan pairs win (EZTV's layout is never
    /// overwritten), then solver clearance, then the saved cookie. Clearance also replaces
    /// the user agent it is bound to. Without either, the plan's headers are unchanged.
    fn headers(
        &self,
        request: &BuiltRequest,
        clearance: Option<&flaresolverr::Clearance>,
    ) -> Result<reqwest::header::HeaderMap> {
        let mut map = headers(request)?;
        if self.saved.is_empty() && clearance.is_none() {
            return Ok(map);
        }
        let mut pairs = plan_cookies(request)?;
        for (name, value) in clearance
            .map(|c| c.cookies.as_slice())
            .unwrap_or_default()
            .iter()
            .chain(&self.saved)
        {
            if !pairs.iter().any(|(n, _)| n == name) {
                pairs.push((name.clone(), value.clone()));
            }
        }
        let joined = pairs
            .iter()
            .map(|(n, v)| format!("{n}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        let mut cookie = reqwest::header::HeaderValue::from_str(&joined)
            .map_err(|_| anyhow!("Unable to compose source cookies"))?;
        cookie.set_sensitive(true);
        map.remove(reqwest::header::COOKIE);
        map.insert(reqwest::header::COOKIE, cookie);
        if let Some(clearance) = clearance {
            map.remove(reqwest::header::USER_AGENT);
            map.insert(reqwest::header::USER_AGENT, clearance.user_agent.clone());
        }
        Ok(map)
    }
}

/// A fully read, same-origin response with the metadata extraction policies need.
pub struct Fetched {
    pub status: u16,
    pub redirected: bool,
    pub refresh: bool,
    /// Final same-origin URL, the base for relative links.
    pub url: url::Url,
    pub content_type: Option<String>,
    /// Raw origin bytes, at most 8 MiB, or the solver's rendered UTF-8 DOM.
    pub body: Vec<u8>,
    /// The body is FlareSolverr's rendered page, not the origin's raw response.
    pub rendered: bool,
    /// The body is UTF-8 text an authenticated session already decoded with the
    /// definition charset; decoding it again would corrupt non-ASCII text.
    pub decoded: bool,
}

/// One source operation sharing the source's rate gate, its epoch state and a deadline.
/// Search sends each planned request through [`Transport::fetch`]; a details-page
/// resolver can send its own same-origin GET the same way.
pub struct Transport<'a> {
    pub client: &'a reqwest::Client,
    pub source: &'a Source,
    pub base: &'a url::Url,
    pub gate: &'a mut Instant,
    pub delay_ms: u64,
    pub deadline: Instant,
}
impl Transport<'_> {
    /// Send one request with the source's cookies. `solvable` declares that the caller can
    /// use rendered UTF-8 HTML; only then, for GET with a configured solver, a recognized
    /// challenge gets exactly one FlareSolverr `request.get`. POST is never replayed.
    pub async fn fetch(&mut self, request: &BuiltRequest, solvable: bool) -> Result<Fetched> {
        if let Some(session) = &self.source.api_session {
            return session
                .fetch_search(self.client, request, self.gate, self.delay_ms)
                .await;
        }
        if let Some(session) = &self.source.cookie_session {
            // HTML GET, or a form POST whose body the request plan already encoded.
            let post = request.method == "POST" && request.body.is_some();
            if !(post || request.method == "GET" && request.body.is_none())
                || !["html", "json", "xml"].contains(&request.response_type.as_str())
            {
                bail!("Unsupported authenticated search request");
            }
            session.check_search_headers(&request.headers)?;
            let url = checked_url(&request.url, self.base)?;
            let body = session
                .fetch_search(
                    url.as_str(),
                    request.body.as_deref(),
                    request.response_type != "html",
                )
                .await?;
            return Ok(Fetched {
                status: 200,
                redirected: false,
                refresh: false,
                url,
                content_type: Some("text/html; charset=utf-8".into()),
                body: body.into_bytes(),
                rendered: false,
                decoded: true,
            });
        }
        let solving = solvable && request.method == "GET" && self.source.solver.is_some();
        let clearance = self.source.clearance();
        let headers = self.source.headers(request, clearance.as_ref())?;
        match send(
            self.client,
            request,
            self.base,
            self.gate,
            self.delay_ms,
            headers,
            solving,
        )
        .await?
        {
            Sent::Response(response, redirected) => {
                let status = response.status().as_u16();
                let refresh = response.headers().contains_key("refresh");
                let url = checked_url(response.url().as_str(), self.base)?;
                let content_type = response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                let body = read_body(response).await?;
                if solvable && request.method == "GET" && flaresolverr::challenge_markers(&body) {
                    if solving {
                        return self.solve(request, url, redirected).await;
                    }
                    bail!("Cloudflare challenge detected; configure the challenge solver");
                }
                Ok(Fetched {
                    status,
                    redirected,
                    refresh,
                    url,
                    content_type,
                    body,
                    rendered: false,
                    decoded: false,
                })
            }
            Sent::Challenge(target, redirected) => self.solve(request, target, redirected).await,
        }
    }
    async fn solve(
        &mut self,
        request: &BuiltRequest,
        target: url::Url,
        redirected: bool,
    ) -> Result<Fetched> {
        let solver = self
            .source
            .solver
            .as_ref()
            .expect("Solving requires a solver");
        let cookies = plan_cookies(request)?;
        // The solver's browser loads the source too: it takes the same gate turn.
        tokio::time::sleep_until((*self.gate).into()).await;
        *self.gate = Instant::now() + Duration::from_millis(self.delay_ms);
        let solved = solver
            .solve(&target, self.base, &cookies, self.deadline)
            .await;
        *self.gate = Instant::now() + Duration::from_millis(self.delay_ms);
        // A failed or unusable solve also discards earlier clearance that just failed.
        let solution = solved.inspect_err(|_| self.source.store(None))?;
        self.source.store(solution.clearance);
        Ok(Fetched {
            status: 200,
            redirected: redirected || solution.url != target,
            refresh: false,
            url: solution.url,
            content_type: None,
            body: solution.body,
            rendered: true,
            decoded: false,
        })
    }
}

/// Resolve a detail page through the same epoch-scoped transport as search.
pub async fn detail_magnet_source(
    policy: DetailMagnet,
    transport: &mut Transport<'_>,
    detail: &url::Url,
) -> Result<String> {
    use anyhow::Context;
    detail_target(transport.base, detail.as_str())?;
    let request = BuiltRequest::html_get(detail.to_string());
    let fetched = transport
        .fetch(&request, true)
        .await
        .context(DetailFailure::Fetch)?;
    let utf8 = fetched.content_type.as_deref().is_none_or(|ct| {
        ct.split(';').skip(1).all(|p| {
            let p = p.trim().to_ascii_lowercase();
            !p.starts_with("charset=")
                || ["charset=utf-8", "charset=\"utf-8\""].contains(&p.as_str())
        })
    });
    if fetched.refresh || !utf8 {
        bail!(DetailFailure::Fetch);
    }
    let page = String::from_utf8(fetched.body).map_err(|_| DetailFailure::Fetch)?;
    let permit = parser_permit().await?;
    Ok(tokio::task::spawn_blocking(move || {
        let _permit = permit;
        select_detail_magnet(policy, &page)
    })
    .await
    .map_err(|_| DetailFailure::Fetch)??)
}
/// Resolve a ticketed detail page through general `download.selectors` (see
/// [`download`]). Public sources use the plain transport; cookie, form and
/// credential-POST sources fetch through their authenticated session. Candidates
/// must be magnets or same-origin links: a cross-origin link is skipped like an
/// upstream selector exception. With `test_link`, a downloaded body that does not
/// start like a bencoded dictionary moves on to the next selector, as upstream does.
pub async fn resolve_download_selectors(
    selectors: std::sync::Arc<download::DownloadSelectors>,
    rt: &Runtime,
    transport: &mut Transport<'_>,
    page_url: &url::Url,
) -> Result<Option<crate::engine::Download>> {
    if transport.source.api_session.is_some() {
        bail!("Download selectors are not supported for API sources");
    }
    let mut before_ctx = rt.plan.context(
        &rt.settings,
        &crate::models::SearchQuery::default(),
        chrono::Utc::now(),
    );
    download::add_download_uri(&mut before_ctx, page_url);
    if let Some(before) = selectors
        .before_request(&before_ctx)
        .map_err(|_| anyhow!("Download request could not be rendered"))?
    {
        let session = transport
            .source
            .cookie_session
            .as_ref()
            .ok_or_else(|| anyhow!("Download before requests need a login session"))?;
        let target = page_url
            .join(&before.path)
            .ok()
            .and_then(|t| checked_url(t.as_str(), transport.base).ok())
            .ok_or_else(|| anyhow!("Download before request target rejected"))?;
        session
            .send_before(target.as_str(), before.fields.as_deref(), page_url.as_str())
            .await?;
    }
    if !selectors.selects() {
        // Upstream then downloads the release link itself.
        return Ok(None);
    }
    let page_request = BuiltRequest::html_get(page_url.to_string());
    let fetched = transport.fetch(&page_request, false).await?;
    if fetched.refresh || !(200..300).contains(&fetched.status) {
        bail!("Source detail page could not be fetched");
    }
    let page = if fetched.decoded || fetched.rendered {
        String::from_utf8(fetched.body).map_err(|_| anyhow!("Invalid detail page text"))?
    } else {
        rt.plan
            .charset()
            .decode(fetched.body)
            .map_err(|_| anyhow!("Only UTF-8 response encoding supported"))?
    };
    let ctx = rt.plan.context(
        &rt.settings,
        &crate::models::SearchQuery::default(),
        chrono::Utc::now(),
    );
    let permit = parser_permit().await?;
    let candidates = tokio::task::spawn_blocking({
        let selectors = selectors.clone();
        move || {
            let _permit = permit;
            selectors.candidates(&page, &ctx)
        }
    })
    .await
    .map_err(|_| anyhow!("Download page parser failed"))?
    .map_err(|_| anyhow!("Download page could not be parsed"))?;
    for candidate in candidates.into_iter().flatten() {
        let Ok(target) = page_url.join(candidate.trim()) else {
            continue;
        };
        if target.scheme() == "magnet" {
            if let Some(magnet) = detail_magnet_uri(target.as_str()) {
                return Ok(Some(crate::engine::Download::Magnet(magnet)));
            }
            continue;
        }
        let Ok(target) = checked_url(target.as_str(), transport.base) else {
            continue;
        };
        let bytes = if let Some(session) = &transport.source.cookie_session {
            session.fetch_download(target.as_str()).await?
        } else {
            let fetched = transport
                .fetch(&BuiltRequest::html_get(target.to_string()), false)
                .await?;
            if fetched.refresh || !(200..300).contains(&fetched.status) {
                continue;
            }
            fetched.body
        };
        if selectors.test_link && bytes.first().is_some_and(|b| *b != b'd') {
            continue;
        }
        return Ok(Some(crate::engine::Download::Torrent {
            bytes,
            content_type: "application/x-bittorrent".into(),
        }));
    }
    bail!("Download selectors did not match")
}
#[cfg(test)]
async fn detail_magnet(
    policy: DetailMagnet,
    client: &reqwest::Client,
    base: &url::Url,
    detail: &url::Url,
    gate: &mut Instant,
    delay_ms: u64,
) -> Result<String> {
    let source = Source::new(&Indexer::default(), None)?;
    detail_magnet_source(
        policy,
        &mut Transport {
            client,
            source: &source,
            base,
            gate,
            delay_ms,
            deadline: Instant::now() + Duration::from_secs(120),
        },
        detail,
    )
    .await
}

/// Search without a challenge solver; the source's saved cookie is still composed.
pub async fn search(
    rt: &Runtime,
    client: &reqwest::Client,
    i: &Indexer,
    q: &SearchQuery,
    gate: &mut Instant,
) -> Result<Vec<Release>> {
    let source = Source::new(i, None)?;
    let deadline = Instant::now() + Duration::from_secs(120);
    search_source(rt, client, i, q, gate, &source, deadline).await
}
/// Search through `source`, the epoch-scoped state of the runtime that owns `gate`.
/// `deadline` is the caller's search deadline; it bounds any challenge solve.
pub async fn search_source(
    rt: &Runtime,
    client: &reqwest::Client,
    i: &Indexer,
    q: &SearchQuery,
    gate: &mut Instant,
    source: &Source,
    deadline: Instant,
) -> Result<Vec<Release>> {
    if i.definition
        .as_ref()
        .and_then(|definition| definition.get("builtin_id"))
        .and_then(serde_json::Value::as_str)
        == Some("milkie")
        && rt
            .settings
            .get("apikey")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|key| key.trim().is_empty())
    {
        bail!("Milkie requires an API key; edit this source and set its apikey before searching");
    }
    rt.validate_query(q)?;
    let mut q = q.clone();
    if q.get("t").is_empty() {
        q.params.insert("t".into(), "search".into());
    }
    let ctx = rt.plan.context(&rt.settings, &q, chrono::Utc::now());
    let requests = rt
        .plan
        .build_requests(&ctx)
        .map_err(|_| anyhow!("Unable to build bundled search requests"))?;
    if requests.len() > 64 {
        bail!("Plan request budget exceeded");
    }
    let base = url::Url::parse(&i.url)?;
    // Validate the entire batch before sending credentials anywhere.
    for r in &requests {
        checked_url(&r.url, &base)?;
        headers(r)?;
        if !["GET", "POST"].contains(&r.method.as_str()) {
            bail!("Unsupported request method");
        }
    }
    let mut results = Vec::new();
    let charset = rt.plan.charset();
    let mut transport = Transport {
        client,
        source,
        base: &base,
        gate,
        delay_ms: rt.delay_ms,
        deadline,
    };
    if let Some((login, url)) = &source.get_login {
        use std::sync::atomic::Ordering;
        if !source.probed.load(Ordering::Acquire) {
            let headers = requests
                .first()
                .map(|r| r.headers.clone())
                .unwrap_or_default();
            let probe = transport
                .fetch(&login.probe_request(url, &headers), false)
                .await?;
            let body = if probe.decoded || probe.rendered {
                String::from_utf8(probe.body).unwrap_or_default()
            } else {
                charset.decode(probe.body).unwrap_or_default()
            };
            login.check(probe.status, &body)?;
            source.probed.store(true, Ordering::Release);
        }
    }
    for mut request in requests {
        // A rendered page is decoded DOM text, usable only as UTF-8 HTML.
        let solvable = request.response_type == "html" && charset == Charset::Utf8;
        let fetched = transport.fetch(&request, solvable).await?;
        let (status, redirected, refresh) = (fetched.status, fetched.redirected, fetched.refresh);
        request.url = fetched.url.to_string();
        // WebResult.Encoding: the definition's Request.Encoding wins over response headers.
        if let Some(ct) = fetched
            .content_type
            .as_deref()
            .filter(|_| charset == Charset::Utf8)
        {
            if ct.split(';').skip(1).any(|s| {
                s.trim().to_ascii_lowercase().starts_with("charset=")
                    && !["charset=utf-8", "charset=\"utf-8\""]
                        .contains(&s.trim().to_ascii_lowercase().as_str())
            }) {
                bail!("Only UTF-8 response encoding supported");
            }
        }
        let text = fetched.decoded || fetched.rendered;
        let bytes = fetched.body;
        let meta = crate::cardigann_exec::ResponseMeta {
            status,
            redirected,
            refresh,
            body_bytes: bytes.len(),
        };
        // The raw cap stays 8 MiB; single-byte decoding can yield up to 24 MiB of UTF-8,
        // which Plan::extract's existing 8 MiB text budget rejects rather than truncates.
        let body = if text {
            String::from_utf8(bytes)
        } else {
            charset.decode(bytes)
        }
        .map_err(|_| anyhow!("Only UTF-8 response encoding supported"))?;
        let permit = parser_permit().await?;
        let plan = rt.plan.clone();
        let context = ctx.clone();
        let container = rt.empty_html_container.clone();
        let dmhy_empty_marker = crate::builtin::id(i).is_ok_and(|id| id == "dmhy");
        let skip_invalid_rows =
            crate::builtin::id(i).is_ok_and(|id| ["1337x", "eztv", "dmhy"].contains(&id));
        // A cancelled blocking job retains its parser permit until its bounded extraction ends.
        let rows = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let rows = if skip_invalid_rows {
                plan.extract_response_skipping_invalid_html_rows(&request, &meta, &body, &context)?
            } else {
                plan.extract_response(&request, &meta, &body, &context)?
            };
            // An empty HTML or XML result must still show the source's result container
            // (e.g. an RSS `channel`); an error page without it is not "no results".
            if rows.is_empty() && ["html", "xml"].contains(&request.response_type.as_str()) {
                if dmhy_empty_marker {
                    // Actual no-match response captured through the protected namespace;
                    // an unrelated table is never evidence of an empty DMHY search.
                    let valid = crate::cardigann_exec::query_leaf_text_equals(
                        &body,
                        r#"div[style="text-align: center;padding: 5px;background-color: #CCDDFF"]"#,
                        "沒有可顯示資源",
                    )
                    .map_err(|_| crate::cardigann_exec::ExecutionError {
                        path: "$.response".into(),
                        message: "Invalid stock empty-result marker".into(),
                    })?;
                    if !valid {
                        return Err(crate::cardigann_exec::ExecutionError {
                            path: "$.response".into(),
                            message: "Expected stock empty-result marker missing".into(),
                        });
                    }
                } else if let Some(container) = container {
                    let selector =
                        scraper::Selector::parse(&container).expect("Validated stock container");
                    if scraper::Html::parse_document(&body)
                        .select(&selector)
                        .next()
                        .is_none()
                    {
                        return Err(crate::cardigann_exec::ExecutionError {
                            path: "$.response".into(),
                            message: "Expected stock result container missing".into(),
                        });
                    }
                }
            }
            Ok(rows)
        })
        .await
        .map_err(|_| anyhow!("Bundled parser failed"))?
        .map_err(|_| anyhow!("Bundled response format or extraction failed"))?;
        results.extend(rows);
        if results.len() >= 1000 {
            results.truncate(1000);
            break;
        }
    }
    *transport.gate = Instant::now() + Duration::from_millis(rt.delay_ms);
    for r in &mut results {
        // Tokyo Toshokan supplies third-party torrent URLs alongside magnets. Prefer
        // its validated magnet; never produce an unusable cross-origin download ticket.
        // Keep this policy local to the fixture-validated preset.
        if crate::builtin::id(i).is_ok_and(|id| id == "tokyotosho") {
            if let Some(magnet) = r.attributes.get("magnet") {
                r.link =
                    detail_magnet_uri(magnet).ok_or_else(|| anyhow!("Invalid release magnet"))?;
            } else {
                checked_url(&r.link, &base)?;
            }
        }
        if r.link.is_empty() {
            if let Some(magnet) = r.attributes.get("magnet") {
                let url = url::Url::parse(magnet).map_err(|_| anyhow!("Invalid release magnet"))?;
                if !btih_magnet(&url) {
                    bail!("Invalid release magnet");
                }
                r.link = magnet.clone();
            }
        }
        if r.link.is_empty()
            && crate::builtin::id(i)
                .is_ok_and(|id| ["therarbg", "linuxtracker", "torrentbyte"].contains(&id))
        {
            // These fixture-tested public definitions have no download URL. Validate the
            // entire value before constructing a URI; never interpret query syntax
            // supplied in a hash. Existing HTTP and magnet targets keep precedence.
            if let Some(hash) = r.attributes.get("infohash") {
                if !((hash.len() == 40 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
                    || (hash.len() == 32
                        && hash
                            .bytes()
                            .all(|b| b.is_ascii_alphabetic() || (b'2'..=b'7').contains(&b))))
                {
                    bail!("Invalid release infohash");
                }
                r.link = format!("magnet:?xt=urn:btih:{hash}");
                r.attributes.insert("magnet".into(), r.link.clone());
            }
        }
        if r.link.is_empty() {
            bail!("Release has no supported download target");
        }
        r.indexer_id = i.id.clone();
        r.indexer = i.name.clone();
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{builtin, cardigann, cardigann_exec::Plan};
    use std::collections::BTreeMap;
    fn runtime(base: &str, search: &str, delay_ms: u64) -> Runtime {
        encoded_runtime(base, search, delay_ms, "UTF-8")
    }
    /// Built directly, bypassing builtin eligibility, which still rejects non-UTF-8.
    fn encoded_runtime(base: &str, search: &str, delay_ms: u64, encoding: &str) -> Runtime {
        let yaml = format!("id: fixture\nname: Fixture\ndescription: Synthetic fixture\nlanguage: en-US\ntype: public\nencoding: {encoding}\nlinks: [{base}]\ncaps:\n  categories: {{1: Other}}\n  modes: {{search: [q]}}\nsettings: []\nsearch:\n{search}\n");
        let root = cardigann::parse(&yaml, cardigann::Limits::default()).unwrap();
        Runtime {
            plan: Arc::new(Plan::compile(&root).unwrap()),
            settings: BTreeMap::new(),
            delay_ms,
            empty_html_container: None,
            modes: BTreeMap::from([("search".into(), vec!["q".into()])]),
            download: None,
        }
    }
    async fn serve(router: axum::Router) -> (String, tokio::task::JoinHandle<()>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let u = format!("http://{}/", l.local_addr().unwrap());
        (
            u,
            tokio::spawn(async move { axum::serve(l, router).await.unwrap() }),
        )
    }
    const JSON_FIELDS:&str="  rows: {selector: items}\n  fields:\n    category: {text: 1}\n    seeders: {text: 1}\n    size: {text: 1024}\n    title: {selector: name}\n    download: {selector: link}";
    #[tokio::test]
    async fn sequential_paths_space_requests_and_post_is_not_retried() {
        use axum::routing::post;
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = seen.clone();
        let app=axum::Router::new().route("/submit",post(move|uri:axum::http::Uri,headers:axum::http::HeaderMap,body:String|{let c=captured.clone();async move {c.lock().unwrap().push((Instant::now(),uri.to_string(),headers["x-auth"].to_str().unwrap().to_owned(),body));axum::Json(serde_json::json!({"items":[{"name":"Generic","link":"magnet:?xt=urn:btih:0123456789012345678901234567890123456789"}]}))}}));
        let (url, server) = serve(app).await;
        let search=format!("  headers: {{X-Auth: ['{{{{ .Config.token }}}}']}}\n  paths:\n    - {{path: submit, method: post, response: {{type: json}}, inputs: {{q: '{{{{ .Keywords }}}}', n: 1}}}}\n    - {{path: submit, method: post, response: {{type: json}}, inputs: {{q: '{{{{ .Keywords }}}}', n: 2}}}}\n{JSON_FIELDS}");
        let mut rt = runtime(&url, &search, 60);
        rt.settings.insert("token".into(), "synthetic-auth".into());
        let i = Indexer {
            url: url.clone(),
            name: "Actual source".into(),
            id: "actual".into(),
            ..Default::default()
        };
        let mut q = SearchQuery::default();
        q.params.insert("q".into(), "A & B".into());
        let rows = search_request(&rt, &i, &q).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].indexer_id, "actual");
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 2);
            assert!(seen[1].0.duration_since(seen[0].0) >= Duration::from_millis(55));
            assert!(seen[0].3.contains("q=A+%26+B"));
            assert_eq!(seen[0].2, "synthetic-auth");
        }
        server.abort();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = calls.clone();
        let (url, server) = serve(axum::Router::new().route(
            "/submit",
            post(move || {
                c.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                async { axum::http::StatusCode::SERVICE_UNAVAILABLE }
            }),
        ))
        .await;
        let rt=runtime(&url,&format!("  paths: [{{path: submit, method: post, response: {{type: json}}}}]\n{JSON_FIELDS}"),0);
        let mut i = i;
        i.url = url;
        assert!(search_request(&rt, &i, &q).await.is_err());
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        server.abort();
    }
    async fn search_request(
        rt: &builtin::Runtime,
        i: &Indexer,
        q: &SearchQuery,
    ) -> Result<Vec<Release>> {
        super::search(
            rt,
            &reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .build()
                .unwrap(),
            i,
            q,
            &mut Instant::now(),
        )
        .await
    }
    #[test]
    fn headers_and_targets_fail_closed() {
        let base = url::Url::parse("https://example.test/").unwrap();
        for raw in [
            "https://other.test/x",
            "https://user:pass@example.test/x",
            "file:///tmp/x",
        ] {
            assert!(checked_url(raw, &base).is_err());
        }
        // Requests are only constructed by plans; header values are then replaced.
        let rt = runtime(base.as_str(), &format!("  path: x\n{HTML_FIELDS}"), 0);
        let ctx = rt
            .plan
            .context(&rt.settings, &SearchQuery::default(), chrono::Utc::now());
        let built = rt.plan.build_requests(&ctx).unwrap().remove(0);
        for (k, v) in [
            ("Host", "example.test"),
            ("Connection", "x"),
            ("X-Auth", "a\rb"),
            ("X-Auth", "a\0b"),
        ] {
            let mut request = built.clone();
            request.headers = vec![(k.into(), v.into())];
            assert!(headers(&request).is_err());
        }
    }
    #[tokio::test]
    async fn upstream_generic_response_fixtures_over_http() {
        for (definition, body, expected_count) in [
            (
                include_str!("../vendor/jackett/fixtures/html-definition1.yml"),
                include_str!("../vendor/jackett/fixtures/html-response1.html"),
                25,
            ),
            (
                include_str!("../vendor/jackett/fixtures/json-definition1.yml"),
                include_str!("../vendor/jackett/fixtures/json-response1.json"),
                78,
            ),
        ] {
            let (url, server) = serve(
                axum::Router::new().fallback(axum::routing::get(move || async move { body })),
            )
            .await;
            let mut root = cardigann::parse(definition, cardigann::Limits::default()).unwrap();
            if let cardigann::Value::Mapping(pairs) = &mut root.value {
                // Response-only harness: no session or download actions are performed.
                pairs.retain(|(k, _)| {
                    ![
                        Some("login"),
                        Some("download"),
                        Some("ratio"),
                        Some("certificates"),
                    ]
                    .contains(&k.text())
                });
                for (k, v) in pairs {
                    if k.text() == Some("links") {
                        if let cardigann::Value::Sequence(links) = &mut v.value {
                            if let cardigann::Value::Scalar(s) = &mut links[0].value {
                                s.text = url.clone();
                            }
                        }
                    }
                }
            }
            let plan = Arc::new(Plan::compile(&root).unwrap());
            assert!(plan.blockers().is_empty());
            let rt = Runtime {
                plan,
                settings: BTreeMap::new(),
                delay_ms: 0,
                modes: BTreeMap::from([("search".into(), vec!["q".into()])]),
                empty_html_container: None,
                download: None,
            };
            let i = Indexer {
                url,
                name: "Fixture configured name".into(),
                id: "fixture-source".into(),
                ..Default::default()
            };
            let q = SearchQuery {
                params: BTreeMap::from([
                    ("q".into(), "ubuntu".into()),
                    ("t".into(), "search".into()),
                ]),
            };
            let ctx = rt.plan.context(&rt.settings, &q, chrono::Utc::now());
            let requests = rt.plan.build_requests(&ctx).unwrap();
            let offline = rt.plan.extract(&requests[0], body, &ctx).unwrap();
            let rows = search_request(&rt, &i, &q).await.unwrap();
            assert_eq!(rows.len(), expected_count);
            assert_eq!(rows.len(), offline.len());
            for (r, expected) in rows.iter().zip(offline) {
                assert_eq!(r.title, expected.title);
                assert_eq!(r.categories, expected.categories);
                assert_eq!(r.size, expected.size);
                assert_eq!(r.indexer_id, i.id);
            }
            server.abort();
        }
    }

    type Seen = Arc<std::sync::Mutex<Vec<String>>>;
    async fn serve_bytes(
        content_type: Option<&'static str>,
        body: Vec<u8>,
    ) -> (String, Seen, tokio::task::JoinHandle<()>) {
        let seen = Seen::default();
        let captured = seen.clone();
        let (url, server) = serve(axum::Router::new().fallback(axum::routing::get(
            move |uri: axum::http::Uri| {
                captured.lock().unwrap().push(uri.to_string());
                let body = body.clone();
                async move {
                    let mut response = axum::response::IntoResponse::into_response(body);
                    response
                        .headers_mut()
                        .remove(axum::http::header::CONTENT_TYPE);
                    if let Some(ct) = content_type {
                        response
                            .headers_mut()
                            .insert(axum::http::header::CONTENT_TYPE, ct.parse().unwrap());
                    }
                    response
                }
            },
        )))
        .await;
        (url, seen, server)
    }
    const HTML_FIELDS: &str = "  rows: {selector: article}\n  fields:\n    category: {text: 1}\n    size: {text: 1024}\n    seeders: {text: 1}\n    title: {selector: h2}\n    download: {selector: a, attribute: href}";
    fn html_body(title: &[u8]) -> Vec<u8> {
        [
            b"<article><h2>".as_slice(),
            title,
            b"</h2><a href='t/1'>Get</a></article>",
        ]
        .concat()
    }
    async fn encoded_search(
        encoding: &str,
        content_type: Option<&'static str>,
        body: Vec<u8>,
        q: &str,
    ) -> (Result<Vec<Release>>, Vec<String>) {
        let (url, seen, server) = serve_bytes(content_type, body).await;
        let search =
            format!("  path: search\n  inputs: {{q: '{{{{ .Keywords }}}}'}}\n{HTML_FIELDS}");
        let rt = encoded_runtime(&url, &search, 0, encoding);
        let i = Indexer {
            url,
            ..Default::default()
        };
        let mut query = SearchQuery::default();
        query.params.insert("q".into(), q.into());
        let result = search_request(&rt, &i, &query).await;
        server.abort();
        let seen = seen.lock().unwrap().clone();
        (result, seen)
    }
    #[test]
    fn latin1_decoder_maps_every_byte_to_the_same_code_point() {
        let all = (0..=255u8).collect::<Vec<_>>();
        let decoded = Charset::decode_latin1(&all);
        assert!(decoded.chars().map(u32::from).eq(0..=255));
        assert_eq!(Charset::decode_latin1(&[0x80, 0x9f]), "\u{80}\u{9f}");
        assert_eq!(Charset::decode_latin1(&[0xef, 0xbb, 0xbf]), "ï»¿");
        assert_eq!(decoded.len(), 128 + 2 * 128);
    }
    #[tokio::test]
    async fn latin1_runtime_uses_definition_encoding_over_headers_and_sniffing() {
        for ct in [
            Some("text/html; charset=iso-8859-1"),
            Some("text/html; charset=utf-8"),
            Some("text/html; charset=windows-1251"),
            Some("text/html"),
            None,
        ] {
            let (rows, seen) =
                encoded_search("iso-8859-1", ct, html_body(b"Caf\xe9\xff"), "café").await;
            assert_eq!(rows.unwrap()[0].title, "Caféÿ", "{ct:?}");
            assert_eq!(seen, ["/search?q=caf%E9"]);
        }
        // No BOM or <meta charset> sniffing: UTF-8 bytes remain Latin-1 mojibake.
        let body = [
            b"\xef\xbb\xbf<meta charset=\"utf-8\">".as_slice(),
            &html_body("é".as_bytes()),
        ]
        .concat();
        let (rows, _) =
            encoded_search("ISO-8859-1", Some("text/html; charset=utf-8"), body, "x").await;
        assert_eq!(rows.unwrap()[0].title, "Ã©");
        // Unrepresentable query text fails before any request is sent.
        for q in ["€", "Ā", "日本", "😀"] {
            let (rows, seen) = encoded_search("iso-8859-1", None, html_body(b"x"), q).await;
            assert!(rows.unwrap_err().to_string().contains("Unable to build"));
            assert!(seen.is_empty(), "{q}");
        }
    }
    #[tokio::test]
    async fn utf8_runtime_decoding_is_unchanged() {
        let (rows, seen) = encoded_search(
            "UTF-8",
            Some("text/html; charset=utf-8"),
            html_body("Café".as_bytes()),
            "café",
        )
        .await;
        assert_eq!(rows.unwrap()[0].title, "Café");
        assert_eq!(seen, ["/search?q=caf%C3%A9"]);
        for (ct, body) in [
            (
                Some("text/html; charset=iso-8859-1"),
                html_body("Café".as_bytes()),
            ),
            (Some("text/html"), html_body(b"Caf\xe9")),
            (None, html_body(b"Caf\xe9")),
        ] {
            let (rows, _) = encoded_search("UTF-8", ct, body, "x").await;
            assert!(rows
                .unwrap_err()
                .to_string()
                .contains("Only UTF-8 response encoding supported"));
        }
        // A UTF-8 BOM is not stripped or treated as an encoding switch.
        let body = [b"\xef\xbb\xbf".as_slice(), &html_body("é".as_bytes())].concat();
        let (rows, _) = encoded_search("utf-8", None, body, "x").await;
        assert_eq!(rows.unwrap()[0].title, "é");
    }
    #[tokio::test]
    async fn same_origin_redirects_preserve_final_base_and_reject_post_conversion() {
        use axum::routing::{get, post};
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = calls.clone();
        let app=axum::Router::new().route("/search",get(||async{axum::response::Redirect::temporary("/browse/")}))
            .route("/browse/",get(move||{seen.fetch_add(1,std::sync::atomic::Ordering::Relaxed);async{axum::response::Html("<article><h2>Redirect fixture</h2><a href='torrent/42'>Get</a></article>")}}))
            .route("/submit",post(||async{(axum::http::StatusCode::FOUND,[("location","/browse/")])}));
        let (url, server) = serve(app).await;
        let fields="  rows: {selector: article}\n  fields:\n    category: {text: 1}\n    size: {text: 1024}\n    seeders: {text: 1}\n    title: {selector: h2}\n    download: {selector: a, attribute: href}";
        let rt = runtime(&url, &format!("  path: search\n{fields}"), 0);
        let i = Indexer {
            url: url.clone(),
            ..Default::default()
        };
        let rows = search_request(&rt, &i, &SearchQuery::default())
            .await
            .unwrap();
        assert_eq!(rows[0].link, format!("{url}browse/torrent/42"));
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        let rt = runtime(
            &url,
            &format!("  paths: [{{path: submit, method: post}}]\n{fields}"),
            0,
        );
        let e = search_request(&rt, &i, &SearchQuery::default())
            .await
            .unwrap_err();
        assert!(e.to_string().contains("method-changing"));
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        server.abort();
    }

    const POLICY: &str = "response: {type: json, noResultsMessage: \"\"}";
    type Calls = Arc<std::sync::Mutex<Vec<String>>>;
    /// Response for a request path and that path's 1-based call number.
    type Respond = fn(&str, usize) -> axum::response::Response;
    /// Every response comes from `respond`, keyed by path and per-path call number.
    async fn serve_policy(respond: Respond) -> (String, Calls, tokio::task::JoinHandle<()>) {
        let calls = Calls::default();
        let seen = calls.clone();
        let (url, server) = serve(axum::Router::new().fallback(move |uri: axum::http::Uri| {
            let seen = seen.clone();
            async move {
                let path = uri.path().to_owned();
                let n = {
                    let mut seen = seen.lock().unwrap();
                    seen.push(uri.to_string());
                    seen.iter()
                        .filter(|p| p.split('?').next() == Some(&path))
                        .count()
                };
                respond(&path, n)
            }
        }))
        .await;
        (url, calls, server)
    }
    fn reply(
        status: u16,
        headers: &[(&'static str, &'static str)],
        body: &'static str,
    ) -> axum::response::Response {
        let mut response = axum::response::IntoResponse::into_response(body);
        *response.status_mut() = axum::http::StatusCode::from_u16(status).unwrap();
        response.headers_mut().clear();
        for (k, v) in headers {
            response.headers_mut().insert(*k, v.parse().unwrap());
        }
        response
    }
    const ROW: &str = r#"{"items":[{"name":"Other path","link":"magnet:?xt=urn:btih:0123456789012345678901234567890123456789"}]}"#;
    async fn policy_search(paths: &str, respond: Respond) -> (Result<Vec<Release>>, Vec<String>) {
        let (url, calls, server) = serve_policy(respond).await;
        // Inherited by every path, so each request URL carries the query.
        let search =
            format!("  inputs: {{q: '{{{{ .Keywords }}}}'}}\n  paths:\n{paths}{JSON_FIELDS}");
        let rt = runtime(&url, &search, 0);
        let i = Indexer {
            url,
            ..Default::default()
        };
        let mut q = SearchQuery::default();
        q.params.insert("q".into(), "synthetic-query".into());
        let result = search_request(&rt, &i, &q).await;
        server.abort();
        let calls = calls.lock().unwrap().clone();
        (result, calls)
    }
    fn policy_path() -> String {
        format!("    - {{path: api, {POLICY}}}\n")
    }
    #[tokio::test]
    async fn empty_no_results_requires_a_direct_zero_byte_http_200() {
        let (rows, calls) = policy_search(&policy_path(), |_, _| reply(200, &[], "")).await;
        assert!(rows.unwrap().is_empty());
        assert_eq!(calls, ["/api?q=synthetic-query"]);
        // Content-Type and Cloudflare server headers alone do not change the outcome.
        let (rows, _) = policy_search(&policy_path(), |_, _| {
            reply(
                200,
                &[
                    ("content-type", "application/json"),
                    ("server", "cloudflare"),
                ],
                "",
            )
        })
        .await;
        assert!(rows.unwrap().is_empty());
        // Nonempty valid JSON is parsed as usual.
        let (rows, _) = policy_search(&policy_path(), |_, _| reply(200, &[], ROW)).await;
        assert_eq!(rows.unwrap()[0].title, "Other path");
        // Decoded BOM, whitespace and malformed bodies are not string.Empty.
        for body in ["\u{feff}", " ", "\n", "{"] {
            let respond: Respond = match body {
                "\u{feff}" => |_, _| reply(200, &[], "\u{feff}"),
                " " => |_, _| reply(200, &[], " "),
                "\n" => |_, _| reply(200, &[], "\n"),
                _ => |_, _| reply(200, &[], "{"),
            };
            let (rows, calls) = policy_search(&policy_path(), respond).await;
            assert!(rows
                .unwrap_err()
                .to_string()
                .contains("Bundled response format or extraction failed"));
            assert_eq!(calls.len(), 1, "{body:?}");
        }
    }
    #[tokio::test]
    async fn empty_no_results_rejects_other_statuses_challenges_and_redirects() {
        let cases: [(Respond, &str, usize); 11] = [
            (|_, _| reply(204, &[], ""), "extraction failed", 1),
            (|_, _| reply(206, &[], ""), "extraction failed", 1),
            (|_, _| reply(201, &[], ""), "extraction failed", 1),
            (|_, _| reply(400, &[], ""), "HTTP 400", 1),
            (|_, _| reply(401, &[], ""), "HTTP 401", 1),
            (|_, _| reply(403, &[], ""), "HTTP 403", 1),
            (|_, _| reply(503, &[], ""), "HTTP 503", 1),
            (
                |_, _| reply(200, &[("cf-mitigated", "challenge")], ""),
                "Cloudflare challenge",
                1,
            ),
            (
                |_, _| reply(403, &[("cf-mitigated", "challenge")], ""),
                "Cloudflare challenge",
                1,
            ),
            (
                |_, _| reply(200, &[("refresh", "0; url=/login")], ""),
                "extraction failed",
                1,
            ),
            // The first response redirects back to the same URL; the second is an empty 200.
            (
                |_, n| {
                    if n == 1 {
                        reply(302, &[("location", "/api?q=synthetic-query")], "")
                    } else {
                        reply(200, &[], "")
                    }
                },
                "extraction failed",
                2,
            ),
        ];
        for (respond, expected, count) in cases {
            let (rows, calls) = policy_search(&policy_path(), respond).await;
            let e = rows.unwrap_err().to_string();
            assert!(e.contains(expected), "{e}");
            assert!(!e.contains("synthetic-query"), "{e}");
            // No retries: every followed hop requests the identical URL.
            assert_eq!(calls, vec!["/api?q=synthetic-query"; count], "{e}");
        }
        // A challenge on a redirect that ends in an empty 200 cannot bypass the policy.
        let (rows, calls) = policy_search(&policy_path(), |_, n| {
            if n == 1 {
                reply(
                    302,
                    &[("location", "/api"), ("cf-mitigated", "challenge")],
                    "",
                )
            } else {
                reply(200, &[], "")
            }
        })
        .await;
        assert!(rows.is_err());
        assert_eq!(calls, ["/api?q=synthetic-query", "/api"]);
        // Redirected nonempty JSON is rejected too; ordinary paths still follow redirects.
        let (rows, _) = policy_search(&policy_path(), |path, _| {
            if path == "/api" {
                reply(302, &[("location", "/final")], "")
            } else {
                reply(200, &[], ROW)
            }
        })
        .await;
        assert!(rows.is_err());
        let (rows, calls) =
            policy_search("    - {path: api, response: {type: json}}\n", |path, _| {
                if path == "/api" {
                    reply(302, &[("location", "/final")], "")
                } else {
                    reply(200, &[], ROW)
                }
            })
            .await;
        assert_eq!(rows.unwrap().len(), 1);
        assert_eq!(calls, ["/api?q=synthetic-query", "/final"]);
        // Existing strict charset handling precedes the policy.
        let (rows, _) = policy_search(&policy_path(), |_, _| {
            reply(
                200,
                &[("content-type", "application/json; charset=iso-8859-1")],
                "",
            )
        })
        .await;
        assert!(rows
            .unwrap_err()
            .to_string()
            .contains("Only UTF-8 response encoding supported"));
    }
    #[tokio::test]
    async fn empty_no_results_skips_only_its_own_path() {
        let paths = format!(
            "    - {{path: empty, {POLICY}}}\n    - {{path: rows, response: {{type: json}}}}\n"
        );
        let (rows, calls) = policy_search(&paths, |path, _| {
            if path == "/empty" {
                reply(200, &[], "")
            } else {
                reply(200, &[], ROW)
            }
        })
        .await;
        let rows = rows.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Other path");
        assert_eq!(
            calls,
            ["/empty?q=synthetic-query", "/rows?q=synthetic-query"]
        );
        // The policy is not inherited: an ordinary path's empty 200 still fails.
        let (rows, calls) = policy_search(&paths, |path, _| {
            if path == "/empty" {
                reply(200, &[], ROW)
            } else {
                reply(200, &[], "")
            }
        })
        .await;
        assert!(rows.is_err());
        assert_eq!(
            calls,
            ["/empty?q=synthetic-query", "/rows?q=synthetic-query"]
        );
    }
    #[tokio::test]
    async fn ordinary_json_status_handling_is_unchanged() {
        let ordinary = "    - {path: api, response: {type: json}}\n";
        // Empty 200 and 204 bodies still fail as invalid JSON; other 2xx JSON is accepted.
        for respond in [(|_, _| reply(200, &[], "")) as Respond, |_, _| {
            reply(204, &[], "")
        }] {
            let (rows, calls) = policy_search(ordinary, respond).await;
            assert!(rows
                .unwrap_err()
                .to_string()
                .contains("Bundled response format or extraction failed"));
            assert_eq!(calls.len(), 1);
        }
        let (rows, _) = policy_search(ordinary, |_, _| reply(201, &[], ROW)).await;
        assert_eq!(rows.unwrap().len(), 1);
        let (rows, _) = policy_search(ordinary, |_, _| {
            reply(200, &[("refresh", "0; url=/login")], ROW)
        })
        .await;
        assert_eq!(rows.unwrap().len(), 1);
    }

    const HASH: &str = "0123456789ABCDEF0123456789ABCDEF01234567";
    const MAGNET: &str = "magnet:?xt=urn:btih:0123456789ABCDEF0123456789ABCDEF01234567&dn=Alpha+S01E02&tr=udp%3A%2F%2Ftracker.example%3A1337%2Fannounce";
    const OTHER: &str = "magnet:?xt=urn:btih:MFRGGZDFMZTWQ2LKNNWG23TPOBYXE43U&dn=Second";
    const ITORRENTS: &str =
        "http://itorrents.org/torrent/0123456789ABCDEF0123456789ABCDEF01234567.torrent";
    const ITORRENTS_FIRST: DetailMagnet = DetailMagnet {
        fallback: crate::builtin::Fallback::ITorrents,
    };
    const MAGNET_FALLBACK: DetailMagnet = DetailMagnet {
        fallback: crate::builtin::Fallback::Magnet,
    };
    /// A 1337x-shaped detail page: list links, with a stray magnet outside any list first.
    fn detail_page(links: &[&str]) -> String {
        let items: String = links
            .iter()
            .map(|href| {
                format!(
                    "<li><a href=\"{}\">Get</a></li>",
                    href.replace('&', "&amp;")
                )
            })
            .collect();
        format!("<!DOCTYPE html><html><body><p><a href=\"{OTHER}\">Related</a></p>\
             <div class=\"torrent-detail-page\"><ul class=\"download-links\">{items}</ul></div></body></html>")
    }
    #[test]
    fn first_primary_magnet_in_document_order_wins() {
        for policy in [ITORRENTS_FIRST, MAGNET_FALLBACK] {
            let select = |links: &[&str]| select_detail_magnet(policy, &detail_page(links));
            assert_eq!(select(&[MAGNET, OTHER]), Ok(MAGNET.into()));
            assert_eq!(select(&[OTHER, MAGNET]), Ok(OTHER.into()));
            // The primary selector skips the iTorrents link even when it comes first.
            assert_eq!(select(&[ITORRENTS, MAGNET]), Ok(MAGNET.into()));
            // Nested markup inside list items still matches the descendant selector.
            let nested = format!("<ul><li><span><a href=\"{OTHER}\">x</a></span></li></ul>");
            assert_eq!(select_detail_magnet(policy, &nested), Ok(OTHER.into()));
        }
    }
    #[test]
    fn fixed_rewrite_applies_to_the_selected_magnet() {
        let tracked = format!(
            "magnet:?xt=urn:btih:{HASH}&tr=http://itorrents.org/announce&tr=http://itorrents.org/b"
        );
        assert_eq!(
            select_detail_magnet(ITORRENTS_FIRST, &detail_page(&[&tracked])),
            Ok(format!("magnet:?xt=urn:btih:{HASH}&tr=https://itorrents.net/announce&tr=https://itorrents.net/b"))
        );
    }
    #[test]
    fn first_invalid_primary_fails_without_trying_later_links() {
        let hex = HASH;
        for bad in [
            "magnet:?xt=urn:btih:0123".to_owned(),
            "magnet:?dn=Only+a+name".to_owned(),
            format!("magnet:?xt=urn:sha1:{hex}"),
            format!("magnet:?xt=urn:btih:{hex}#fragment"),
            format!("magnet://user:secret@tracker.example/?xt=urn:btih:{hex}"),
            format!("magnet:path?xt=urn:btih:{hex}"),
            format!("javascript:alert('magnet:?xt=urn:btih:{hex}')"),
            format!("/redirect?to=magnet:?xt=urn:btih:{hex}"),
            format!("https://1337x.to/go/magnet:?xt=urn:btih:{hex}"),
            format!(" magnet:?xt=urn:btih:{hex}"),
            format!("magnet:?xt=urn:btih:{hex}\n&dn=split"),
            format!("magnet:?xt=urn:btih:{hex}&dn={}", "x".repeat(16 * 1024)),
        ] {
            for policy in [ITORRENTS_FIRST, MAGNET_FALLBACK] {
                assert_eq!(
                    select_detail_magnet(policy, &detail_page(&[&bad, MAGNET])),
                    Err(DetailFailure::InvalidMagnet),
                    "{bad}"
                );
            }
        }
    }
    #[test]
    fn missing_primary_never_falls_back_to_a_fetch() {
        let only_itorrents = detail_page(&[ITORRENTS]);
        assert_eq!(
            select_detail_magnet(ITORRENTS_FIRST, &only_itorrents),
            Err(DetailFailure::FallbackUnsupported)
        );
        assert_eq!(
            select_detail_magnet(MAGNET_FALLBACK, &only_itorrents),
            Err(DetailFailure::NoMagnet)
        );
        // The stray magnet outside any list is never selected.
        for policy in [ITORRENTS_FIRST, MAGNET_FALLBACK] {
            assert_eq!(
                select_detail_magnet(policy, &detail_page(&[])),
                Err(DetailFailure::NoMagnet)
            );
            assert_eq!(
                select_detail_magnet(policy, ""),
                Err(DetailFailure::NoMagnet)
            );
            assert_eq!(
                select_detail_magnet(policy, "{\"magnet\":\"magnet:?xt=urn:btih:0\"}"),
                Err(DetailFailure::NoMagnet)
            );
        }
        // Pages beyond the search document bounds fail instead of being partly searched.
        let deep = format!("{}{}", "<div>".repeat(200), detail_page(&[MAGNET]));
        assert_eq!(
            select_detail_magnet(ITORRENTS_FIRST, &deep),
            Err(DetailFailure::Fetch)
        );
    }
    #[test]
    fn detail_targets_are_same_origin_torrent_pages_only() {
        let base = url::Url::parse("https://1337x.to/").unwrap();
        assert_eq!(
            detail_target(&base, "https://1337x.to/torrent/1/Alpha/")
                .unwrap()
                .as_str(),
            "https://1337x.to/torrent/1/Alpha/"
        );
        for raw in [
            "https://1337x.to/torrent/1/Alpha/?x=1",
            "https://1337x.to/torrent/1/Alpha/#x",
            "https://1337x.to/user/someone/",
            "https://1337x.to/",
            "https://u:p@1337x.to/torrent/1/Alpha/",
            "http://1337x.to/torrent/1/Alpha/",
            "https://1337x.st/torrent/1/Alpha/",
            "https://itorrents.net/torrent/1/Alpha/",
            "magnet:?xt=urn:btih:0123456789ABCDEF0123456789ABCDEF01234567",
            "file:///torrent/1",
        ] {
            assert_eq!(
                detail_target(&base, raw),
                Err(DetailFailure::Target),
                "{raw}"
            );
        }
    }
    fn page_response() -> axum::response::Response {
        static PAGE: OnceLock<&'static str> = OnceLock::new();
        reply(
            200,
            &[("content-type", "text/html; charset=utf-8")],
            PAGE.get_or_init(|| Box::leak(detail_page(&[MAGNET]).into_boxed_str())),
        )
    }
    async fn resolve(respond: Respond, delay_ms: u64) -> (Result<String>, Vec<String>) {
        let (url, calls, server) = serve_policy(respond).await;
        let base = url::Url::parse(&url).unwrap();
        let detail = detail_target(&base, &format!("{url}torrent/1/Alpha/")).unwrap();
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .unwrap();
        let result = detail_magnet(
            ITORRENTS_FIRST,
            &client,
            &base,
            &detail,
            &mut Instant::now(),
            delay_ms,
        )
        .await;
        server.abort();
        let calls = calls.lock().unwrap().clone();
        (result, calls)
    }
    fn assert_fetch_failure(result: Result<String>) {
        let e = result.unwrap_err();
        assert_eq!(
            e.downcast_ref::<DetailFailure>(),
            Some(&DetailFailure::Fetch)
        );
        assert_eq!(e.to_string(), "Source detail page could not be fetched");
    }
    #[tokio::test]
    async fn detail_page_fetch_resolves_through_same_origin_redirects() {
        let (magnet, calls) = resolve(|_, _| page_response(), 0).await;
        assert_eq!(magnet.unwrap(), MAGNET);
        assert_eq!(calls, ["/torrent/1/Alpha/"]);
        let (magnet, calls) = resolve(
            |path, _| {
                if path == "/torrent/1/Alpha/" {
                    reply(301, &[("location", "/torrent/1/Alpha-renamed/")], "")
                } else {
                    page_response()
                }
            },
            0,
        )
        .await;
        assert_eq!(magnet.unwrap(), MAGNET);
        assert_eq!(calls, ["/torrent/1/Alpha/", "/torrent/1/Alpha-renamed/"]);
    }
    #[tokio::test]
    async fn detail_page_failures_are_fixed_and_unretried() {
        static BIG: OnceLock<&'static str> = OnceLock::new();
        let cases: [(Respond, usize); 10] = [
            (|_, _| reply(404, &[], "not found"), 1),
            (|_, _| reply(503, &[], ""), 1),
            (
                |_, _| reply(403, &[("cf-mitigated", "challenge")], "Just a moment"),
                1,
            ),
            (
                |_, _| reply(200, &[("cf-mitigated", "challenge")], "Just a moment"),
                1,
            ),
            (
                |_, _| reply(200, &[("refresh", "0; url=/torrent/1/Alpha/")], "x"),
                1,
            ),
            (
                |_, _| {
                    reply(200, &[("content-type", "text/html; charset=iso-8859-1")], "<ul><li><a href='magnet:?xt=urn:btih:0123456789ABCDEF0123456789ABCDEF01234567'>m</a></li></ul>")
                },
                1,
            ),
            (
                |_, _| {
                    reply(
                        200,
                        &[],
                        BIG.get_or_init(|| {
                            Box::leak(
                                format!(
                                    "{}{}",
                                    detail_page(&[MAGNET]),
                                    " ".repeat(8 * 1024 * 1024)
                                )
                                .into_boxed_str(),
                            )
                        }),
                    )
                },
                1,
            ),
            (
                |_, _| reply(302, &[("location", "javascript:alert(1)")], ""),
                1,
            ),
            (
                |_, _| {
                    reply(
                        302,
                        &[("location", "https://itorrents.net/torrent/x.torrent")],
                        "",
                    )
                },
                1,
            ),
            // Six hops: the redirect budget ends the loop.
            (
                |_, _| reply(302, &[("location", "/torrent/1/Alpha/")], ""),
                6,
            ),
        ];
        for (respond, count) in cases {
            let (result, calls) = resolve(respond, 0).await;
            assert_eq!(calls, vec!["/torrent/1/Alpha/"; count]);
            assert_fetch_failure(result);
        }
        let (result, _) = resolve(|_, _| reply(200, &[], "<p>Nothing here</p>"), 0).await;
        assert_eq!(
            result.unwrap_err().downcast_ref::<DetailFailure>(),
            Some(&DetailFailure::NoMagnet)
        );
    }
    #[tokio::test]
    async fn cross_origin_redirect_is_never_followed_and_hops_are_spaced() {
        let other_hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hits = other_hits.clone();
        let (other, other_server) = serve(axum::Router::new().fallback(move || {
            hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async { axum::response::Html(detail_page(&[MAGNET])) }
        }))
        .await;
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        let (url, server) = serve(axum::Router::new().fallback(move |uri: axum::http::Uri| {
            let log = log.clone();
            let other = other.clone();
            async move {
                log.lock()
                    .unwrap()
                    .push((Instant::now(), uri.path().to_owned()));
                match uri.path() {
                    "/torrent/1/Alpha/" => reply(302, &[("location", "/torrent/1/Beta/")], ""),
                    "/torrent/1/Beta/" => {
                        let mut r = reply(302, &[], "");
                        r.headers_mut().insert(
                            "location",
                            format!("{other}torrent/1/Beta/").parse().unwrap(),
                        );
                        r
                    }
                    _ => page_response(),
                }
            }
        }))
        .await;
        let base = url::Url::parse(&url).unwrap();
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let mut gate = Instant::now();
        let detail = detail_target(&base, &format!("{url}torrent/1/Alpha/")).unwrap();
        let result = detail_magnet(ITORRENTS_FIRST, &client, &base, &detail, &mut gate, 60).await;
        assert_fetch_failure(result);
        let seen = seen.lock().unwrap().clone();
        assert_eq!(
            seen.iter().map(|(_, p)| p.as_str()).collect::<Vec<_>>(),
            ["/torrent/1/Alpha/", "/torrent/1/Beta/"]
        );
        assert!(seen[1].0.duration_since(seen[0].0) >= Duration::from_millis(55));
        assert!(gate > seen[1].0);
        assert_eq!(other_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
        server.abort();
        other_server.abort();
    }
}
