//! Cardigann `login: {method: get}` for API-key sources (pinned `CardigannIndexer.cs`
//! DoLogin `get` branch and `checkForError` at `:538-562`).
//!
//! Upstream sends one GET to `path?inputs` without cookies, fails on HTTP 401 or on any
//! declared error selector matching the response (parsed as HTML), and keeps the
//! response cookies. None of the admitted definitions declares a positive `test`, and
//! each sends its key in every search query as well. Rackett therefore runs the GET as a
//! one-time probe per source runtime before its first search, with the same checks.
//! Difference from upstream: response cookies of the probe are not retained, because
//! every admitted search already carries its key. A source that relied on a session
//! cookie would fail its searches visibly, not silently.
//!
//! Admitted subset: login keys `method`, `path`, `inputs` and `error` only; a literal
//! path; inputs that are literals or a declared text/password setting; no `test`.
//! Search headers (often the key, e.g. `Authorization: Bearer {{ .Config.apikey }}`)
//! are rendered by the request plan and sent with the probe and with downloads.
use crate::cardigann::Node;
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::collections::BTreeMap;

enum Input {
    Literal(String),
    Setting(String),
}

pub struct GetLogin {
    path: String,
    inputs: Vec<(String, Input)>,
    errors: Vec<String>,
}

fn keys(node: &Node, allowed: &[&str]) -> Result<()> {
    let pairs = node.pairs().ok_or_else(|| anyhow!("Expected a mapping"))?;
    let mut seen = std::collections::BTreeSet::new();
    for (k, _) in pairs {
        let k = k.text().ok_or_else(|| anyhow!("Invalid key"))?;
        if !allowed.contains(&k) || !seen.insert(k) {
            bail!("Unsupported GET login property");
        }
    }
    Ok(())
}

impl GetLogin {
    pub fn compile(root: &Node) -> Result<Self> {
        let login = root.get("login").ok_or_else(|| anyhow!("No login"))?;
        keys(login, &["method", "path", "inputs", "error"])?;
        if login.string("method") != Some("get") {
            bail!("Not a GET login");
        }
        if root.get("followredirect").is_some() || super::auth::search_follows_redirects(root) {
            bail!("GET login transport semantics are not modeled");
        }
        let path = login
            .string("path")
            .filter(|p| {
                !p.is_empty()
                    && p.len() <= 2048
                    && !p.contains("{{")
                    && !p.contains(['?', '#'])
                    && !p.chars().any(char::is_control)
            })
            .ok_or_else(|| anyhow!("Unsupported GET login path"))?
            .to_owned();
        let declared: BTreeMap<_, _> = root
            .get("settings")
            .and_then(Node::items)
            .unwrap_or_default()
            .iter()
            .filter_map(|s| {
                s.string("name")
                    .map(|n| (n, s.string("type").unwrap_or("text")))
            })
            .collect();
        let mut inputs = Vec::new();
        for (name, value) in login
            .get("inputs")
            .and_then(Node::pairs)
            .unwrap_or_default()
        {
            let name = name
                .text()
                .filter(|n| !n.is_empty() && n.len() <= 256 && !n.chars().any(char::is_control))
                .ok_or_else(|| anyhow!("Invalid GET login input"))?;
            let value = value
                .text()
                .ok_or_else(|| anyhow!("Invalid GET login input"))?;
            let input = match value
                .strip_prefix("{{ .Config.")
                .and_then(|v| v.strip_suffix(" }}"))
            {
                Some(key) if matches!(declared.get(key), Some(&"text" | &"password")) => {
                    Input::Setting(key.to_owned())
                }
                Some(_) => bail!("GET login input requires a declared text setting"),
                None if !value.contains(['{', '}']) && value.len() <= 8192 => {
                    Input::Literal(value.to_owned())
                }
                None => bail!("Unsupported GET login input template"),
            };
            if inputs.len() >= 32 {
                bail!("GET login input limit exceeded");
            }
            inputs.push((name.to_owned(), input));
        }
        // Upstream sends Login.Headers ?? Search.Headers with the login GET; the request
        // plan renders them (see `probe_request`). A key there also counts.
        let header_secret = root
            .get("search")
            .and_then(|s| s.get("headers"))
            .and_then(Node::pairs)
            .is_some_and(|h| {
                h.iter().any(|(_, v)| {
                    v.items()
                        .unwrap_or_default()
                        .iter()
                        .chain(std::iter::once(v))
                        .any(|v| v.text().is_some_and(|t| t.contains("{{ .Config.")))
                })
            });
        if !header_secret && !inputs.iter().any(|(_, i)| matches!(i, Input::Setting(_))) {
            // A probe without any secret would validate nothing.
            bail!("GET login declares no credential input");
        }
        let mut errors = Vec::new();
        if let Some(blocks) = login.get("error") {
            let blocks = blocks
                .items()
                .filter(|b| b.len() <= 8)
                .ok_or_else(|| anyhow!("Unsupported GET login error list"))?;
            for block in blocks {
                // Upstream raises on the matching selector; messages are not exposed.
                keys(block, &["selector", "message"])?;
                let selector = block
                    .string("selector")
                    .filter(|s| !s.is_empty() && s.len() <= 1024 && !s.contains("{{"))
                    .ok_or_else(|| anyhow!("Unsupported GET login error selector"))?;
                crate::cardigann_exec::css::Css::parse(selector).map_err(|e| anyhow!(e))?;
                errors.push(selector.to_owned());
            }
        }
        Ok(Self {
            path,
            inputs,
            errors,
        })
    }
    /// The probe URL: `resolvePath(path + "?" + GetQueryString())`, where each value is
    /// `WebUtilityHelpers.UrlEncode(value, UTF8)` and keys are not encoded.
    pub fn url(&self, base: &url::Url, settings: &BTreeMap<String, Value>) -> Result<url::Url> {
        let mut query = String::new();
        for (name, input) in &self.inputs {
            let value = match input {
                Input::Literal(v) => v.clone(),
                Input::Setting(k) => match settings.get(k) {
                    Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
                    _ => bail!("This preset requires its {k} setting"),
                },
            };
            if !query.is_empty() {
                query.push('&');
            }
            query.push_str(name);
            query.push('=');
            query.push_str(&crate::cardigann_exec::filters::url_encode(&value));
        }
        let target = base
            .join(&format!("{}?{query}", self.path))
            .map_err(|_| anyhow!("Invalid GET login path"))?;
        super::checked_url(target.as_str(), base)
    }
    /// The probe as a request carrying the plan's rendered search headers.
    pub fn probe_request(
        &self,
        url: &url::Url,
        search_headers: &[(String, String)],
    ) -> crate::cardigann_exec::request::BuiltRequest {
        let mut request = crate::cardigann_exec::request::BuiltRequest::html_get(url.to_string());
        request.headers = search_headers.to_vec();
        request
    }
    /// `checkForError`: 401 fails; any declared error selector present fails.
    pub fn check(&self, status: u16, body: &str) -> Result<()> {
        if status == 401 {
            bail!("Source rejected the configured key (HTTP 401)");
        }
        for selector in &self.errors {
            if crate::cardigann_exec::query_selector_exists(body, selector).unwrap_or(true) {
                bail!("Source rejected the configured key");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{builtin, cardigann};
    fn root(id: &str) -> Node {
        let raw = builtin::SOURCES.iter().find(|(i, _)| *i == id).unwrap().1;
        cardigann::parse(raw, cardigann::Limits::default()).unwrap()
    }
    #[test]
    fn pinned_api_key_probes_compile_and_header_or_test_shapes_do_not() {
        for id in ["binarybay", "bitagent", "anthelion-api", "swarmazon-api"] {
            assert!(GetLogin::compile(&root(id)).is_ok(), "{id}");
        }
        // Keys in search headers count as the credential.
        for id in ["digitalcore-api", "flood", "alabala-api"] {
            assert!(GetLogin::compile(&root(id)).is_ok(), "{id}");
        }
        // No GET login, or a templated login path.
        for id in ["1337x", "chdbits", "scenetime-api", "v3x"] {
            assert!(GetLogin::compile(&root(id)).is_err(), "{id}");
        }
    }
    #[test]
    fn probe_url_encodes_values_and_checks_errors() {
        let login = GetLogin::compile(&root("binarybay")).unwrap();
        let base = url::Url::parse("https://binarybay.test/").unwrap();
        let settings = BTreeMap::from([("apikey".to_owned(), Value::String("k y&1".into()))]);
        let url = login.url(&base, &settings).unwrap();
        assert_eq!(url.path(), "/torznab/api");
        assert_eq!(url.query(), Some("t=caps&apikey=k+y%261"));
        assert!(login.url(&base, &BTreeMap::new()).is_err());
        assert!(login.check(200, "<caps></caps>").is_ok());
        assert!(login.check(401, "").is_err());
        assert!(login
            .check(200, "<error code=\"100\" description=\"Invalid key\"/>")
            .is_err());
    }
}
