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
//! Admitted subset: login keys `method`, `path`, `inputs`, `error` and `headers`; no
//! `test`. The path is site-relative; after its `?` it may substitute declared
//! text/password settings, rendered raw as upstream does (`applyGoTemplateText(Login.Path,
//! variables)`, `:827`, no URL-encoding modifier). Such a value must consist of URL
//! unreserved characters, so the raw and an encoded rendering agree and it cannot change
//! the URL's structure. Inputs are literals or a declared text/password setting.
//! Search headers (often the key, e.g. `Authorization: Bearer {{ .Config.apikey }}`)
//! are rendered by the request plan and sent with the probe and with downloads; a
//! `login.headers` block is admitted only when it equals the search headers, because
//! upstream sends `Login.Headers ?? Search.Headers` (`:572`).
use crate::cardigann::Node;
use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use std::collections::BTreeMap;

enum Input {
    Literal(String),
    Setting(String),
}

pub struct GetLogin {
    /// Literal text and raw-rendered settings, in order (`:827`).
    path: Vec<Input>,
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

/// Structural equality of two definition nodes, ignoring source locations and quoting.
fn same(a: &Node, b: &Node) -> bool {
    use crate::cardigann::Value as V;
    match (&a.value, &b.value) {
        (V::Scalar(x), V::Scalar(y)) => x.text == y.text,
        (V::Sequence(x), V::Sequence(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y))
        }
        (V::Mapping(x), V::Mapping(y)) => {
            x.len() == y.len()
                && x.iter()
                    .zip(y)
                    .all(|((xk, xv), (yk, yv))| same(xk, yk) && same(xv, yv))
        }
        _ => false,
    }
}
/// A raw-rendered path setting: non-empty URL unreserved characters (RFC 3986), which
/// `applyGoTemplateText` and `WebUtility.UrlEncode` would both leave unchanged.
fn unreserved(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 1024
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
}
impl GetLogin {
    pub fn compile(root: &Node) -> Result<Self> {
        let login = root.get("login").ok_or_else(|| anyhow!("No login"))?;
        keys(login, &["method", "path", "inputs", "error", "headers"])?;
        if login.string("method") != Some("get") {
            bail!("Not a GET login");
        }
        // Definition-level `followredirect` matters upstream only to TestLogin (`:923`),
        // which needs a `test`, to the form landing page (`:1032`), and to a search
        // redirected to another domain (`CheckIfLoginIsNeeded`, `:951-969`), where it
        // rewrites the stored site link and still fails the request. Rackett fails that
        // request too (redirects are pinned to the origin) and never rewrites the URL.
        if !matches!(root.string("followredirect"), None | Some("true" | "false"))
            || super::auth::search_follows_redirects(root)
        {
            bail!("GET login transport semantics are not modeled");
        }
        if let Some(headers) = login.get("headers") {
            let search = root.get("search").and_then(|s| s.get("headers"));
            if !search.is_some_and(|s| same(headers, s)) {
                bail!("GET login headers must equal the search headers");
            }
        }
        let raw = login
            .string("path")
            .filter(|p| {
                !p.is_empty()
                    && p.len() <= 2048
                    && !p.contains('#')
                    && !p.chars().any(char::is_control)
            })
            .ok_or_else(|| anyhow!("Unsupported GET login path"))?;
        let lower = raw.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") || raw.starts_with("//") {
            // The probe, like the search, would carry the key to another host
            // (docs/get-login-paths.md).
            bail!("GET login on another host needs a credentialed API origin");
        }
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
        let setting = |key: &str| match declared.get(key) {
            Some(&"text" | &"password") => Ok(key.to_owned()),
            _ => Err(anyhow!("GET login input requires a declared text setting")),
        };
        let literal = |l: &str| {
            if l.contains(char::is_whitespace) {
                bail!("Unsupported GET login path");
            }
            Ok(Input::Literal(l.to_owned()))
        };
        let mut path = Vec::new();
        let mut rest = raw;
        let mut query = false;
        while let Some(start) = rest.find("{{") {
            // Settings are substituted only into the query part.
            query |= rest[..start].contains('?');
            if !query {
                bail!("Unsupported GET login path");
            }
            let end = rest[start..]
                .find("}}")
                .ok_or_else(|| anyhow!("Unsupported GET login path template"))?
                + start;
            let key = rest[start..end + 2]
                .strip_prefix("{{ .Config.")
                .and_then(|k| k.strip_suffix(" }}"))
                .ok_or_else(|| anyhow!("Unsupported GET login path template"))?;
            if start > 0 {
                path.push(literal(&rest[..start])?);
            }
            path.push(Input::Setting(setting(key)?));
            rest = &rest[end + 2..];
        }
        if rest.contains(['{', '}']) {
            bail!("Unsupported GET login path template");
        }
        if !rest.is_empty() {
            path.push(literal(rest)?);
        }
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
                Some(key) => Input::Setting(setting(key)?),
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
        if !header_secret
            && !inputs
                .iter()
                .map(|(_, i)| i)
                .chain(&path)
                .any(|i| matches!(i, Input::Setting(_)))
        {
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
    /// The probe URL: `resolvePath(path + "?" + GetQueryString())` (`:827`), where each
    /// value is `WebUtilityHelpers.UrlEncode(value, UTF8)` and keys are not encoded. The
    /// `?` is appended even when the path already has a query, as upstream does.
    pub fn url(&self, base: &url::Url, settings: &BTreeMap<String, Value>) -> Result<url::Url> {
        let value = |input: &Input| match input {
            Input::Literal(v) => Ok(v.clone()),
            Input::Setting(k) => match settings.get(k) {
                Some(Value::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
                _ => Err(anyhow!("This preset requires its {k} setting")),
            },
        };
        let mut path = String::new();
        for input in &self.path {
            let v = value(input)?;
            if let Input::Setting(k) = input {
                if !unreserved(&v) {
                    bail!("This preset's {k} setting may contain only letters, digits and - . _ ~");
                }
            }
            path.push_str(&v);
        }
        let mut query = String::new();
        for (name, input) in &self.inputs {
            let value = value(input)?;
            if !query.is_empty() {
                query.push('&');
            }
            query.push_str(name);
            query.push('=');
            query.push_str(&crate::cardigann_exec::filters::url_encode(&value));
        }
        let target = base
            .join(&format!("{path}?{query}"))
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
        // A templated query in the path, and login headers equal to the search headers.
        for id in ["scenetime-api", "hdforever-api"] {
            assert!(GetLogin::compile(&root(id)).is_ok(), "{id}");
        }
        // No GET login, or a login on an API host.
        for id in ["1337x", "chdbits", "abnormal-api", "v3x", "yggreborn-api"] {
            assert!(GetLogin::compile(&root(id)).is_err(), "{id}");
        }
    }
    fn compile(yaml: &str) -> Result<GetLogin> {
        let raw = format!(
            "settings:\n  - {{name: apikey, type: text}}\n  - {{name: sort, type: select}}\n{yaml}"
        );
        GetLogin::compile(&cardigann::parse(&raw, cardigann::Limits::default()).unwrap())
    }
    #[test]
    fn templated_paths_render_raw_unreserved_settings_in_the_query_only() {
        let login = GetLogin::compile(&root("scenetime-api")).unwrap();
        let base = url::Url::parse("https://scenetime.test/").unwrap();
        let key = |k: &str| BTreeMap::from([("apikey".to_owned(), Value::String(k.into()))]);
        // `path + "?" + ""` (`:827`): upstream's extra `?` stays in the last value.
        let url = login.url(&base, &key("Ab-1_2.3~")).unwrap();
        assert_eq!(url.path(), "/rss.php");
        assert_eq!(url.query(), Some("apikey=Ab-1_2.3~&view=html?"));
        for bad in ["a&b", "a#b", "a b", "a/b", "a%41", "\u{e9}", " ", ""] {
            let err = login.url(&base, &key(bad)).unwrap_err().to_string();
            // Errors name the setting, never its value.
            assert!(err.contains("apikey") && (bad.trim().is_empty() || !err.contains(bad)));
        }
        assert!(login.url(&base, &BTreeMap::new()).is_err());
        assert!(compile("login: {method: get, path: \"a?k={{ .Config.apikey }}\"}\n").is_ok());
        for path in [
            "{{ .Config.apikey }}/a",
            "a/{{ .Config.apikey }}?x=1",
            "a?k={{ .Config.sort }}",
            "a?k={{ .Config.missing }}",
            "a?k={{ if .Config.apikey }}1{{ end }}",
            "a?k={{.Config.apikey}}",
            "a?k={{ .Config.apikey }",
            "a?k={{ .Config.apikey }}#f",
            "a?k={{ .Config.apikey }} x",
            "https://{{ .Config.apikey }}/a",
            "//api.test/a?k={{ .Config.apikey }}",
        ] {
            let yaml = format!("login: {{method: get, path: \"{path}\"}}\n");
            assert!(compile(&yaml).is_err(), "{path}");
        }
    }
    #[test]
    fn login_headers_must_equal_the_search_headers_and_followredirect_is_inert() {
        let login =
            "login: {method: get, path: a, headers: {X-Api-Key: [\"{{ .Config.apikey }}\"]}}\n";
        let same = "search: {headers: {X-Api-Key: ['{{ .Config.apikey }}']}}\n";
        assert!(compile(&format!("{login}{same}")).is_ok());
        for search in [
            "",
            "search: {headers: {X-Api-Key: [\"{{ .Config.sort }}\"]}}\n",
            "search: {headers: {X-Other: [\"{{ .Config.apikey }}\"]}}\n",
            "search: {headers: {X-Api-Key: [\"{{ .Config.apikey }}\", x]}}\n",
        ] {
            assert!(compile(&format!("{login}{search}")).is_err(), "{search}");
        }
        let path = "login: {method: get, path: \"a?k={{ .Config.apikey }}\"}\n";
        assert!(compile(&format!("followredirect: true\n{path}")).is_ok());
        assert!(compile(&format!("followredirect: maybe\n{path}")).is_err());
        let search = "search: {paths: [{path: x, followredirect: true}]}\n";
        assert!(compile(&format!("{path}{search}")).is_err());
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
