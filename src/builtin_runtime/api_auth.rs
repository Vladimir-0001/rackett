//! Source-pinned UNIT3D/NexusPHP GET authentication. A bounded preflight precedes each
//! uncached search/download; its response cookies live only for that operation.
//! API keys, URLs and remote body text never appear in diagnostics.
use crate::{cardigann::Node, cardigann_exec::request::BuiltRequest};
use anyhow::{anyhow, bail, Result};
use futures_util::StreamExt;
use reqwest::{
    header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE, COOKIE, LOCATION, REFERER},
    Client,
};
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

fn keys(n: &Node, expected: &[&str]) -> bool {
    n.pairs().is_some_and(|pairs| {
        pairs.len() == expected.len()
            && pairs
                .iter()
                .filter_map(|(k, _)| k.text())
                .collect::<BTreeSet<_>>()
                == expected.iter().copied().collect()
    })
}
fn unique(n: &Node) -> bool {
    if let Some(pairs) = n.pairs() {
        let mut seen = BTreeSet::new();
        pairs
            .iter()
            .all(|(k, v)| k.text().is_some_and(|k| seen.insert(k)) && unique(v))
    } else {
        n.items().is_none_or(|items| items.iter().all(unique))
    }
}
const SELECTORS: [&str; 2] = [
    "a[href*=\"/login\"]",
    ":root:contains(\"Account is Banned\")",
];
const NEXUS_SELECTORS: [&str; 2] = [
    ":root:contains(\"login.php?returnto\")",
    ":root:contains(\"Unauthenticated\")",
];
#[derive(Clone, Copy)]
enum Family {
    Unit3d,
    NexusPhp,
}
impl Family {
    fn login(self) -> &'static str {
        match self {
            Self::Unit3d => "api/torrents",
            Self::NexusPhp => "api/v1/torrents?per_page=1",
        }
    }
    fn search(self) -> &'static str {
        match self {
            Self::Unit3d => "api/torrents/filter",
            Self::NexusPhp => "api/v1/torrents",
        }
    }
    fn selectors(self) -> [&'static str; 2] {
        match self {
            Self::Unit3d => SELECTORS,
            Self::NexusPhp => NEXUS_SELECTORS,
        }
    }
}
/// Accept the complete pinned login/transport shape, never a method-name waiver.
/// Catalog admission is separate and remains an explicit provider list.
pub fn validate_definition(root: &Node) -> Result<()> {
    compile(root).map(|_| ())
}
fn compile(root: &Node) -> Result<Family> {
    let valid = (|| {
        if !unique(root)
            || root.string("encoding") != Some("UTF-8")
            || root.get("followredirect").is_some()
            || super::auth::search_follows_redirects(root)
            || root.get("download").is_some()
        {
            return None;
        }
        let login = root.get("login")?;
        if login.string("method") != Some("get") {
            return None;
        }
        let family = match login.string("path")? {
            "api/torrents" if keys(login, &["path", "method", "error"]) => Family::Unit3d,
            "api/v1/torrents" if keys(login, &["path", "method", "inputs", "error"]) => {
                let inputs = login.get("inputs")?;
                if !keys(inputs, &["per_page"]) || inputs.string("per_page") != Some("1") {
                    return None;
                }
                Family::NexusPhp
            }
            _ => return None,
        };
        let errors = login.get("error")?.items()?;
        if errors.len() != 2
            || !keys(&errors[0], &["selector", "message"])
            || !keys(
                &errors[1],
                match family {
                    Family::Unit3d => &["selector"],
                    Family::NexusPhp => &["selector", "message"],
                },
            )
            || errors
                .iter()
                .zip(family.selectors())
                .any(|(e, s)| e.string("selector") != Some(s))
        {
            return None;
        }
        let count = match family {
            Family::Unit3d => 1,
            Family::NexusPhp => 2,
        };
        let expected = match family {
            Family::Unit3d => "The API key was not accepted by {{ .Config.sitelink }}.",
            Family::NexusPhp => "The API token was not accepted by {{ .Config.sitelink }}.",
        };
        for error in errors.iter().take(count) {
            let message = error.get("message")?;
            if !keys(message, &["text"]) || message.string("text") != Some(expected) {
                return None;
            }
        }
        let search = root.get("search")?;
        if search.get("method").is_some() || search.get("response").is_some() {
            return None;
        }
        let paths = search.get("paths")?.items()?;
        if paths.len() != 1
            || !keys(&paths[0], &["path", "response"])
            || paths[0].string("path") != Some(family.search())
        {
            return None;
        }
        let response = paths[0].get("response")?;
        if !keys(response, &["type"]) || response.string("type") != Some("json") {
            return None;
        }
        let headers = search.get("headers")?;
        if !keys(headers, &["Authorization"]) {
            return None;
        }
        let values = headers.get("Authorization")?.items()?;
        if values.len() != 1 || values[0].text() != Some("Bearer {{ .Config.apikey }}") {
            return None;
        }
        let rows = search.get("rows")?;
        let rows_valid = match family {
            Family::Unit3d => keys(rows, &["selector", "attribute"])
                && rows.string("selector") == Some("data")
                && rows.string("attribute") == Some("attributes"),
            Family::NexusPhp => keys(rows, &["selector"])
                && rows.string("selector") == Some("data.data{{ if .Config.freeleech }}:not(promotion_info.down_multiplier:contains(1)):not(promotion_info.down_multiplier:contains(.)){{ else }}{{ end }}"),
        };
        if !rows_valid {
            return None;
        }
        let settings = root.get("settings")?.items()?;
        for name in match family {
            Family::Unit3d => &["apikey"][..],
            Family::NexusPhp => &["apikey", "passkey"][..],
        } {
            let mut keys = settings.iter().filter(|s| s.string("name") == Some(name));
            if keys.next()?.string("type") != Some("text") || keys.next().is_some() {
                return None;
            }
        }
        Some(family)
    })();
    valid.ok_or_else(|| anyhow!("Unsupported API authentication schema"))
}
/// No Debug: the bearer value is a secret. The key stays inside one source epoch.
pub struct ApiSession {
    base: url::Url,
    authorization: HeaderValue,
    family: Family,
}
impl ApiSession {
    pub fn new(root: &Node, base: &str, key: &str) -> Result<Self> {
        let family = compile(root)?;
        let base = url::Url::parse(base).map_err(|_| anyhow!("Invalid API source URL"))?;
        super::checked_url(base.as_str(), &base)?;
        if base.query().is_some() || base.fragment().is_some() {
            bail!("Invalid API source URL");
        }
        if key.is_empty() || key.len() > 4096 || key.bytes().any(|b| !b.is_ascii_graphic()) {
            bail!("API preset requires a nonempty API key without whitespace");
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {key}"))
            .map_err(|_| anyhow!("Invalid API key"))?;
        authorization.set_sensitive(true);
        Ok(Self {
            base,
            authorization,
            family,
        })
    }
    fn target(&self, raw: &str) -> Result<url::Url> {
        let u = self
            .base
            .join(raw)
            .map_err(|_| anyhow!("Invalid API target"))?;
        super::checked_url(u.as_str(), &self.base)?;
        if u.fragment().is_some() {
            bail!("API target fragment rejected");
        }
        Ok(u)
    }
    async fn receive(
        &self,
        client: &Client,
        target: &url::Url,
        cookies: &mut super::cookies::Cookies,
        gate: &mut Instant,
        delay_ms: u64,
        download: bool,
    ) -> Result<super::Fetched> {
        let mut url = target.clone();
        let hops = if download { 5 } else { 0 };
        let limit = if download {
            32 * 1024 * 1024
        } else {
            8 * 1024 * 1024
        };
        for hop in 0..=hops {
            crate::engine::wait_until(*gate).await;
            *gate = Instant::now() + Duration::from_millis(delay_ms);
            let mut request = client
                .get(url.clone())
                .header(AUTHORIZATION, self.authorization.clone())
                .header(REFERER, self.base.as_str());
            if let Some(cookie) = cookies.header(&url, chrono::Utc::now().timestamp())? {
                request = request.header(COOKIE, cookie);
            }
            let response = request
                .send()
                .await
                .map_err(|_| anyhow!("API source transport failed"))?;
            cookies.update(&url, response.headers(), chrono::Utc::now().timestamp())?;
            if response.headers().contains_key("refresh")
                || response
                    .headers()
                    .get("cf-mitigated")
                    .is_some_and(|v| v == "challenge")
            {
                bail!("API source redirect or challenge rejected");
            }
            if response.status().is_redirection() {
                if hop == hops || ![301, 302, 303, 307, 308].contains(&response.status().as_u16()) {
                    bail!("API source redirect rejected");
                }
                let location = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| anyhow!("Invalid API redirect"))?;
                url = self.target(
                    url.join(location)
                        .map_err(|_| anyhow!("Invalid API redirect"))?
                        .as_str(),
                )?;
                continue;
            }
            if response.status().as_u16() != 200 {
                bail!("API source returned HTTP {}", response.status().as_u16());
            }
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            if response.content_length().is_some_and(|n| n > limit as u64) {
                bail!("API response exceeds byte budget");
            }
            let mut bytes = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| anyhow!("Unable to read API response"))?;
                if bytes.len() + chunk.len() > limit {
                    bail!("API response exceeds byte budget");
                }
                bytes.extend_from_slice(&chunk);
            }
            *gate = Instant::now() + Duration::from_millis(delay_ms);
            return Ok(super::Fetched {
                status: 200,
                redirected: hop > 0,
                refresh: false,
                url,
                content_type,
                body: bytes,
                rendered: false,
                decoded: false,
            });
        }
        unreachable!()
    }
    async fn login(
        &self,
        client: &Client,
        gate: &mut Instant,
        delay_ms: u64,
    ) -> Result<super::cookies::Cookies> {
        // Pinned DoLogin clears cookies before the GET. No stale response cookies
        // or second source's credentials can enter this operation.
        let mut cookies = super::cookies::Cookies::empty(&self.base);
        let response = self
            .receive(
                client,
                &self.target(self.family.login())?,
                &mut cookies,
                gate,
                delay_ms,
                false,
            )
            .await?;
        if response.content_type.as_deref().is_some_and(|ct| {
            ct.split(';').skip(1).any(|p| {
                let p = p.trim().to_ascii_lowercase();
                p.starts_with("charset=")
                    && !["charset=utf-8", "charset=\"utf-8\""].contains(&p.as_str())
            })
        }) {
            bail!("API authentication requires UTF-8");
        }
        let text = String::from_utf8(response.body)
            .map_err(|_| anyhow!("API authentication requires UTF-8"))?;
        let permit = super::parser_permit().await?;
        let family = self.family;
        tokio::task::spawn_blocking(move || -> Result<()> {
            let _permit = permit;
            for selector in family.selectors() {
                if crate::cardigann_exec::query_selector_exists(&text, selector)
                    .map_err(|_| anyhow!("API authentication document unsupported"))?
                {
                    bail!("API authentication rejected");
                }
            }
            let json: serde_json::Value = serde_json::from_str(&text)
                .map_err(|_| anyhow!("API authentication did not return JSON"))?;
            let data = match family {
                Family::Unit3d => json.get("data"),
                Family::NexusPhp => json.get("data").and_then(|v| v.get("data")),
            };
            if !data.is_some_and(serde_json::Value::is_array) {
                bail!("API authentication data missing");
            }
            Ok(())
        })
        .await
        .map_err(|_| anyhow!("API authentication parser failed"))??;
        Ok(cookies)
    }
    pub async fn fetch_search(
        &self,
        client: &Client,
        request: &BuiltRequest,
        gate: &mut Instant,
        delay_ms: u64,
    ) -> Result<super::Fetched> {
        let url = self.target(&request.url)?;
        if request.method != "GET"
            || request.body.is_some()
            || request.response_type != "json"
            || request.headers.len() != 1
            || !request
                .headers
                .iter()
                .all(|(k, v)| k == "Authorization" && v.as_bytes() == self.authorization.as_bytes())
            || url.path() != self.target(self.family.search())?.path()
        {
            bail!("Unsupported authenticated API request");
        }
        let mut cookies = self.login(client, gate, delay_ms).await?;
        self.receive(client, &url, &mut cookies, gate, delay_ms, false)
            .await
    }
    pub async fn fetch_download(
        &self,
        client: &Client,
        target: &str,
        gate: &mut Instant,
        delay_ms: u64,
    ) -> Result<Vec<u8>> {
        let url = self.target(target)?;
        let mut cookies = self.login(client, gate, delay_ms).await?;
        Ok(self
            .receive(client, &url, &mut cookies, gate, delay_ms, true)
            .await?
            .body)
    }
}
