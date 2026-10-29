//! Opt-in FlareSolverr fallback for recognized Cloudflare challenges on bundled sources.
//!
//! Default off: only an explicit `RACKETT_FLARESOLVERR_URL` enables it; there is no
//! implicit endpoint. The endpoint is a trusted, operator-chosen service. It receives the
//! challenged same-origin URL and the definition's own static Cookie header pairs, never
//! saved source cookies, API keys or other request headers. Request and response shapes
//! follow FlareSolverr 3.5.2 (`src/dtos.py`, `src/flaresolverr_service.py`): `POST /v1`
//! with `cmd: request.get`, `url`, `maxTimeout` (ms) and optional `cookies`; a reply with
//! `status: ok|error` and `solution.{url,status,response,cookies,userAgent}`, where
//! `response` is the browser's rendered DOM and `cookies` are WebDriver cookies.
use super::auth::cookie_pair_valid;
use anyhow::{anyhow, bail, Result};
use futures_util::StreamExt;
use reqwest::header::{HeaderValue, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::{Arc, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const ENV: &str = "RACKETT_FLARESOLVERR_URL";
/// Explicit source egress family for deployments whose solver uses a different route.
/// This does not change how Rackett connects to the solver itself.
pub const SOURCE_IP_FAMILY_ENV: &str = "RACKETT_FLARESOLVERR_SOURCE_IP_FAMILY";
fn source_address(value: &str) -> Result<Option<IpAddr>> {
    match value {
        "" | "auto" => Ok(None),
        "ipv4" => Ok(Some(Ipv4Addr::UNSPECIFIED.into())),
        "ipv6" => Ok(Some(Ipv6Addr::UNSPECIFIED.into())),
        _ => bail!("Invalid {SOURCE_IP_FAMILY_ENV}; expected auto, ipv4 or ipv6"),
    }
}
/// Upper bound for one solve, as FlareSolverr's own default `maxTimeout`.
pub const MAX_TIMEOUT_MS: u64 = 60_000;
/// Rendered page cap, identical to the raw search response cap.
const BODY_LIMIT: usize = 8 * 1024 * 1024;
/// JSON escaping (`\"`, `\n`, `\uXXXX`) can expand an 8 MiB page; the envelope gets 16 MiB.
const ENVELOPE_LIMIT: usize = 16 * 1024 * 1024;
/// Challenge interstitials are small; markers are searched in this prefix only.
const MARKER_WINDOW: usize = 256 * 1024;
/// Deadline share kept back for reading and parsing the solver's reply.
const MARGIN: Duration = Duration::from_secs(1);
const MIN_SOLVE: Duration = Duration::from_secs(1);

/// The validated solver endpoint and its own non-redirecting, proxy-free client.
pub struct Solver {
    endpoint: url::Url,
    client: reqwest::Client,
    source_address: Option<IpAddr>,
}
impl Solver {
    /// Accepts `http(s)://host[:port]/` or `.../v1` without credentials, query or fragment.
    pub fn new(raw: &str) -> Result<Self> {
        let invalid = || {
            anyhow!("Invalid {ENV}; expected http(s)://host:port/ without credentials, query or fragment")
        };
        let mut endpoint = url::Url::parse(raw).map_err(|_| invalid())?;
        if !matches!(endpoint.scheme(), "http" | "https")
            || endpoint.host_str().is_none()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || !["/", "/v1"].contains(&endpoint.path())
        {
            return Err(invalid());
        }
        endpoint.set_path("/v1");
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .connect_timeout(Duration::from_secs(10))
            .user_agent("Rackett/0.1")
            .build()
            .map_err(|_| anyhow!("Challenge solver client creation failed"))?;
        Ok(Self {
            endpoint,
            client,
            source_address: None,
        })
    }
    pub(crate) fn source_client(&self, client: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
        client.local_address(self.source_address)
    }
    /// The process-wide solver from [`ENV`], read once; unset or empty disables it.
    pub fn configured() -> Result<Option<Arc<Self>>> {
        static SOLVER: OnceLock<Result<Option<Arc<Solver>>, String>> = OnceLock::new();
        SOLVER
            .get_or_init(|| {
                let family = std::env::var(SOURCE_IP_FAMILY_ENV)
                    .or_else(|e| match e {
                        std::env::VarError::NotPresent => Ok(String::new()),
                        _ => Err(e),
                    })
                    .map_err(|_| format!("Invalid {SOURCE_IP_FAMILY_ENV}"))?;
                let address = source_address(&family).map_err(|e| e.to_string())?;
                match std::env::var(ENV) {
                    Ok(raw) if !raw.is_empty() => Self::new(&raw)
                        .map(|mut s| {
                            s.source_address = address;
                            Some(Arc::new(s))
                        })
                        .map_err(|e| e.to_string()),
                    Ok(_) | Err(std::env::VarError::NotPresent) => Ok(None),
                    Err(_) => Err(format!("Invalid {ENV}")),
                }
            })
            .clone()
            .map_err(|e| anyhow!(e))
    }
    /// One `request.get` for `target`. Bounded by `deadline` and [`MAX_TIMEOUT_MS`];
    /// never retried. Errors are static local messages.
    pub(super) async fn solve(
        &self,
        target: &url::Url,
        base: &url::Url,
        cookies: &[(String, String)],
        deadline: Instant,
    ) -> Result<Solution> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let budget = remaining
            .checked_sub(MARGIN)
            .filter(|budget| *budget >= MIN_SOLVE)
            .ok_or_else(|| {
                anyhow!("Search deadline leaves no time to solve the source challenge; raise the request timeout")
            })?;
        let body = serde_json::to_vec(&Request {
            cmd: "request.get",
            url: target.as_str(),
            max_timeout: budget.as_millis().min(MAX_TIMEOUT_MS as u128) as u64,
            cookies: cookies
                .iter()
                .map(|(name, value)| Cookie { name, value })
                .collect(),
        })?;
        let response = self
            .client
            .post(self.endpoint.clone())
            .header(CONTENT_TYPE, "application/json")
            .timeout(remaining)
            .body(body)
            .send()
            .await
            .map_err(|_| anyhow!("Challenge solver transport failed (connection or timeout)"))?;
        if response.status() != reqwest::StatusCode::OK {
            bail!("Challenge solver could not solve the source challenge");
        }
        if response
            .content_length()
            .is_some_and(|n| n > ENVELOPE_LIMIT as u64)
        {
            bail!("Challenge solver response exceeds its size limit");
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| anyhow!("Unable to read challenge solver response"))?;
            if bytes.len() + chunk.len() > ENVELOPE_LIMIT {
                bail!("Challenge solver response exceeds its size limit");
            }
            bytes.extend_from_slice(&chunk);
        }
        let malformed = || anyhow!("Challenge solver returned a malformed response");
        let envelope: Envelope = serde_json::from_slice(&bytes).map_err(|_| malformed())?;
        if envelope.status != "ok" {
            bail!("Challenge solver could not solve the source challenge");
        }
        let solution = envelope.solution.ok_or_else(malformed)?;
        if solution.status != 200 {
            bail!("Challenge solver page returned an HTTP error");
        }
        let url = super::checked_url(&solution.url, base)
            .map_err(|_| anyhow!("Challenge solver ended on another origin"))?;
        let page = solution
            .response
            .filter(|p| !p.is_empty())
            .ok_or_else(malformed)?;
        if page.len() > BODY_LIMIT {
            bail!("Response exceeds 8 MiB");
        }
        if challenge_markers(page.as_bytes()) {
            bail!("Cloudflare challenge was not solved by the challenge solver");
        }
        Ok(Solution {
            url,
            body: page.into_bytes(),
            clearance: Clearance::from_solution(&solution.cookies, solution.user_agent, base),
        })
    }
}

#[derive(Serialize)]
struct Request<'a> {
    cmd: &'static str,
    url: &'a str,
    #[serde(rename = "maxTimeout")]
    max_timeout: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    cookies: Vec<Cookie<'a>>,
}
#[derive(Serialize)]
struct Cookie<'a> {
    name: &'a str,
    value: &'a str,
}
#[derive(Deserialize)]
struct Envelope {
    status: String,
    solution: Option<SolutionIn>,
}
#[derive(Deserialize)]
struct SolutionIn {
    url: String,
    status: u16,
    response: Option<String>,
    #[serde(default)]
    cookies: Vec<WebDriverCookie>,
    #[serde(rename = "userAgent")]
    user_agent: Option<String>,
}
#[derive(Deserialize)]
struct WebDriverCookie {
    name: String,
    value: String,
    domain: Option<String>,
    path: Option<String>,
    secure: Option<bool>,
    expiry: Option<f64>,
}

pub(super) struct Solution {
    pub url: url::Url,
    pub body: Vec<u8>,
    pub clearance: Option<Clearance>,
}

/// Solver cookies and the user agent they are bound to, for one source epoch only.
#[derive(Clone)]
pub(super) struct Clearance {
    pub cookies: Vec<(String, String)>,
    pub user_agent: HeaderValue,
    expires: Option<SystemTime>,
}
impl Clearance {
    /// Keep only cookies that a request to the exact source origin would send at any path:
    /// matching host (a leading-dot domain cookie is narrowed to the host itself), path `/`,
    /// `secure` only over HTTPS, unexpired, and an ordinary RFC6265 pair. Nothing is kept
    /// without a valid user agent, since clearance cookies are bound to it.
    fn from_solution(
        cookies: &[WebDriverCookie],
        user_agent: Option<String>,
        base: &url::Url,
    ) -> Option<Self> {
        let host = base.host_str()?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_secs_f64();
        let mut kept = Vec::new();
        let mut names = BTreeSet::new();
        let mut size = 0;
        let mut expires: Option<f64> = None;
        for c in cookies.iter().take(64) {
            let domain = c.domain.as_deref().unwrap_or("");
            if domain
                .strip_prefix('.')
                .unwrap_or(domain)
                .eq_ignore_ascii_case(host)
                && c.path.as_deref().is_none_or(|p| p == "/")
                && (!c.secure.unwrap_or(false) || base.scheme() == "https")
                && c.expiry.is_none_or(|e| {
                    e > now
                        && Duration::try_from_secs_f64(e)
                            .ok()
                            .and_then(|duration| UNIX_EPOCH.checked_add(duration))
                            .is_some()
                })
                && cookie_pair_valid(&c.name, &c.value)
                && size + c.name.len() + c.value.len() + 2 <= 4096
                && names.insert(c.name.as_str())
            {
                size += c.name.len() + c.value.len() + 2;
                kept.push((c.name.clone(), c.value.clone()));
                if let Some(e) = c.expiry {
                    expires = Some(expires.map_or(e, |x| x.min(e)));
                }
            }
        }
        let user_agent = user_agent
            .filter(|ua| {
                (1..=512).contains(&ua.len()) && ua.bytes().all(|b| (0x20..0x7f).contains(&b))
            })
            .and_then(|ua| HeaderValue::from_str(&ua).ok())?;
        (!kept.is_empty()).then(|| Self {
            cookies: kept,
            user_agent,
            expires: expires
                .and_then(|e| Duration::try_from_secs_f64(e).ok())
                .and_then(|duration| UNIX_EPOCH.checked_add(duration)),
        })
    }
    pub fn fresh(&self) -> bool {
        self.expires.is_none_or(|e| SystemTime::now() < e)
    }
}

/// Bounded Cloudflare interstitial markers (FlareSolverr's `CHALLENGE_TITLES` plus the
/// challenge-only `_cf_chl_opt` and `/cdn-cgi/challenge-platform/h/` orchestration). The
/// `/cdn-cgi/challenge-platform/scripts/` probe injected into ordinary pages is not one.
pub(super) fn challenge_markers(body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(&body[..body.len().min(MARKER_WINDOW)]).to_ascii_lowercase();
    [
        "<title>just a moment...</title>",
        "window._cf_chl_opt",
        "/cdn-cgi/challenge-platform/h/",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

#[cfg(test)]
mod family_tests {
    use super::*;

    #[test]
    fn source_family_is_explicit_and_invalid_values_are_redacted() {
        assert_eq!(source_address("").unwrap(), None);
        assert_eq!(source_address("auto").unwrap(), None);
        assert_eq!(
            source_address("ipv4").unwrap(),
            Some(Ipv4Addr::UNSPECIFIED.into())
        );
        assert_eq!(
            source_address("ipv6").unwrap(),
            Some(Ipv6Addr::UNSPECIFIED.into())
        );
        for value in ["IPV4", "ipv4 ", "localhost", "private-secret-value"] {
            assert_eq!(
                source_address(value).unwrap_err().to_string(),
                "Invalid RACKETT_FLARESOLVERR_SOURCE_IP_FAMILY; expected auto, ipv4 or ipv6"
            );
        }
    }
}
