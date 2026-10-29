//! Immutable offline requests, following CardigannIndexer.PerformQuery input ordering.
use super::template::{Context, Template};
use crate::cardigann::{Node, Scalar, ScalarStyle, Value};
use std::collections::BTreeSet;
use url::Url;

const MAX_REQUESTS: usize = 64;
const MAX_INPUTS: usize = 256;
const MAX_REQUEST_BYTES: usize = 128 * 1024;

/// Definition text encoding. CardigannIndexer calls Encoding.GetEncoding(Definition.Encoding),
/// defaulting to UTF-8. Only labels whose .NET semantics are exactly modeled are admitted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Charset {
    #[default]
    Utf8,
    /// .NET Latin1Encoding: code points U+0000..=U+00FF map to the identical byte.
    Latin1,
}
impl Charset {
    pub fn from_definition(root: &Node) -> Result<Self, String> {
        let Some(node) = root.get("encoding") else {
            return Ok(Self::Utf8);
        };
        let encoding = node
            .text()
            .ok_or("$.encoding: unsupported non-scalar encoding")?;
        if encoding.eq_ignore_ascii_case("utf-8") || encoding.eq_ignore_ascii_case("utf8") {
            Ok(Self::Utf8)
        } else if encoding.eq_ignore_ascii_case("iso-8859-1") {
            Ok(Self::Latin1)
        } else {
            Err(format!("$.encoding: unsupported encoding {encoding}"))
        }
    }
    /// WebUtilityHelpers.UrlEncode(value, encoding). Unrepresentable Latin-1 text is an
    /// explicit error rather than .NET's '?' replacement fallback.
    pub(crate) fn url_encode(self, value: &str) -> Result<String, String> {
        match self {
            Self::Utf8 => Ok(super::filters::url_encode(value)),
            Self::Latin1 => value
                .chars()
                .map(|c| u8::try_from(u32::from(c)).ok())
                .collect::<Option<Vec<_>>>()
                .map(|bytes| super::filters::url_encode_bytes(&bytes))
                .ok_or_else(|| "value contains text not representable in ISO-8859-1".into()),
        }
    }
    /// Encoding.GetString for a response: every Latin-1 byte maps to the same code point,
    /// with no BOM or meta sniffing. Output may be twice the input length in UTF-8.
    pub fn decode_latin1(bytes: &[u8]) -> String {
        bytes.iter().copied().map(char::from).collect()
    }
}
#[derive(Clone, Debug)]
pub struct RequestPlan {
    base: Url,
    charset: Charset,
    paths: Vec<PathPlan>,
    headers: Vec<(String, Template)>,
    allow_empty: bool,
}
#[derive(Clone, Debug)]
struct PathPlan {
    path: Template,
    inputs: Vec<(String, Template, String)>,
    categories: Vec<String>,
    method: String,
    response_type: String,
    empty_no_results: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltRequest {
    pub url: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub response_type: String,
    /// Private so only RequestPlan::build can attach a path's no-results policy.
    empty_no_results: bool,
}
impl BuiltRequest {
    /// A header-free HTML GET outside any search path, as for a detail page.
    pub fn html_get(url: String) -> Self {
        Self {
            url,
            method: "GET".into(),
            headers: Vec::new(),
            body: None,
            response_type: "html".into(),
            empty_no_results: false,
        }
    }
    /// This JSON path declared `response.noResultsMessage: ""`: a trusted HTTP 200 response
    /// with zero body bytes and no followed redirect contributes no results.
    pub fn empty_no_results(&self) -> bool {
        self.empty_no_results
    }
}
fn scalar<'a>(node: &'a Node, path: &str) -> Result<&'a str, String> {
    node.text()
        .ok_or_else(|| format!("{path}: expected scalar"))
}
fn boolean(node: Option<&Node>, default: bool, path: &str) -> Result<bool, String> {
    match node.map(|n| scalar(n, path)).transpose()? {
        None => Ok(default),
        Some("true") => Ok(true),
        Some("false") => Ok(false),
        _ => Err(format!("{path}: expected true or false")),
    }
}
fn inputs(node: Option<&Node>, path: &str) -> Result<Vec<(String, Template, String)>, String> {
    let Some(node) = node else {
        return Ok(Vec::new());
    };
    let pairs = node
        .pairs()
        .ok_or_else(|| format!("{path}: expected mapping"))?;
    if pairs.len() > MAX_INPUTS {
        return Err(format!("{path}: too many inputs"));
    }
    pairs
        .iter()
        .map(|(key, value)| {
            let key = scalar(key, path)?;
            if key != "$raw"
                && (key.is_empty()
                    || !key
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-._[]!()*$".contains(&b)))
            {
                return Err(format!(
                    "{path}.{key}: unsupported unsafe literal input key"
                ));
            }
            Ok((
                key.to_owned(),
                Template::compile(scalar(value, path)?)
                    .map_err(|e| format!("{path}.{key}: {e}"))?,
                format!("{path}.{key}"),
            ))
        })
        .collect()
}
fn repeated_key(node: &Node) -> bool {
    let mut seen = BTreeSet::new();
    node.pairs()
        .unwrap_or_default()
        .iter()
        .any(|(k, _)| !seen.insert(k.text().unwrap_or_default()))
}
/// Compilation reads the first of repeated keys, which could hide another search, path list,
/// path or response block. When any occurrence along root -> search -> paths[i] -> response
/// declares noResultsMessage, every mapping on that chain must have unique keys. Mappings
/// below it (inputs, fields, headers) keep their ordered repeated-key semantics. The walk is
/// a fixed four levels over already parsed, size-limited nodes.
fn unambiguous_policy_structure(root: &Node) -> Result<(), String> {
    let mut declared = false;
    let mut ambiguous = repeated_key(root).then(|| "$".to_owned());
    let mut note = |node: &Node, location: &dyn Fn() -> String| {
        if ambiguous.is_none() && repeated_key(node) {
            ambiguous = Some(location());
        }
    };
    for search in root.all("search") {
        note(search, &|| "$.search".into());
        let mut responses: Vec<(&Node, String)> = search
            .all("response")
            .map(|r| (r, "$.search.response".to_owned()))
            .collect();
        for paths in search.all("paths") {
            for (i, path) in paths.items().unwrap_or_default().iter().enumerate() {
                note(path, &|| format!("$.search.paths[{i}]"));
                responses.extend(
                    path.all("response")
                        .map(|r| (r, format!("$.search.paths[{i}].response"))),
                );
            }
        }
        for (response, location) in responses {
            note(response, &|| location.clone());
            declared |= response.all("noResultsMessage").next().is_some();
        }
    }
    match ambiguous {
        Some(location) if declared => Err(format!(
            "{location}: repeated structural property with noResultsMessage"
        )),
        _ => Ok(()),
    }
}
// FormUrlEncodedContent uses RFC3986 escaping, with spaces rendered as '+'.
fn form_encode(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            output.push(byte as char);
        } else if byte == b' ' {
            output.push('+');
        } else {
            use std::fmt::Write;
            write!(&mut output, "%{byte:02X}").expect("string write");
        }
    }
    output
}
impl RequestPlan {
    pub fn compile(root: &Node) -> Result<Self, String> {
        unambiguous_policy_structure(root)?;
        let charset = Charset::from_definition(root)?;
        let link = root
            .get("links")
            .and_then(Node::items)
            .and_then(|v| v.first())
            .ok_or("$.links: missing base URL")?;
        let base =
            Url::parse(scalar(link, "$.links[0]")?).map_err(|e| format!("$.links[0]: {e}"))?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
        {
            return Err("$.links[0]: expected HTTP(S) base without credentials".into());
        }
        let search = root.get("search").ok_or("$.search: missing")?;
        let common = search.get("inputs");
        let mut paths = Vec::new();
        let path_nodes = search.get("paths").and_then(Node::items);
        let nodes: Vec<&Node> = if let Some(nodes) = path_nodes {
            nodes.iter().collect()
        } else {
            vec![search]
        };
        if nodes.is_empty() {
            return Err("$.search.paths: no paths".into());
        }
        if nodes.len() > MAX_REQUESTS {
            return Err("$.search.paths: too many paths".into());
        }
        for (i, node) in nodes.into_iter().enumerate() {
            let prefix = if path_nodes.is_some() {
                format!("$.search.paths[{i}]")
            } else {
                "$.search".into()
            };
            if path_nodes.is_some() {
                for (key, _) in node
                    .pairs()
                    .ok_or_else(|| format!("{prefix}: expected mapping"))?
                {
                    let key = scalar(key, &prefix)?;
                    if !matches!(
                        key,
                        "path"
                            | "method"
                            | "inputs"
                            | "queryseparator"
                            | "categories"
                            | "inheritinputs"
                            | "followredirect"
                            | "response"
                    ) {
                        return Err(format!("{prefix}.{key}: unsupported path semantics"));
                    }
                }
            }
            let mut empty_no_results = false;
            if let Some(response) = node.get("response") {
                let pairs = response
                    .pairs()
                    .ok_or_else(|| format!("{prefix}.response: expected mapping"))?;
                for (key, value) in pairs {
                    let key = scalar(key, &prefix)?;
                    if key == "noResultsMessage" && path_nodes.is_some() {
                        empty_no_results = true;
                        // CardigannIndexer.cs:1640-1645 continues only on exactly HTTP 200;
                        // nonempty messages are an ordinal Contains and stay unsupported.
                        if !matches!(
                            &value.value,
                            Value::Scalar(Scalar { text, style: ScalarStyle::SingleQuoted | ScalarStyle::DoubleQuoted })
                                if text.is_empty()
                        ) {
                            return Err(format!(
                                "{prefix}.response.noResultsMessage: only an exact quoted empty string is supported"
                            ));
                        }
                    } else if key != "type" {
                        return Err(format!(
                            "{prefix}.response.{key}: unsupported response semantics"
                        ));
                    }
                }
            }
            if node.string("queryseparator").is_some_and(|v| v != "&") {
                return Err(format!(
                    "{prefix}.queryseparator: unsupported for upstream search requests"
                ));
            }
            if boolean(node.get("followredirect"), false, &prefix)? {
                return Err(format!(
                    "{prefix}.followredirect: redirect execution requires session integration"
                ));
            }
            let mut fields =
                if path_nodes.is_some() && boolean(node.get("inheritinputs"), true, &prefix)? {
                    inputs(common, "$.search.inputs")?
                } else {
                    Vec::new()
                };
            fields.extend(inputs(node.get("inputs"), &format!("{prefix}.inputs"))?);
            if fields.len() > MAX_INPUTS {
                return Err(format!("{prefix}.inputs: too many inherited inputs"));
            }
            let method = node.string("method").unwrap_or("get").to_ascii_uppercase();
            if !matches!(method.as_str(), "GET" | "POST") {
                return Err(format!("{prefix}.method: unsupported {method}"));
            }
            // Upstream non-UTF-8 FormUrlEncodedContent semantics differ and are not modeled.
            if method == "POST" && charset != Charset::Utf8 {
                return Err(format!(
                    "{prefix}.method: unsupported POST with non-UTF-8 encoding"
                ));
            }
            let response_type = node
                .get("response")
                .and_then(|n| n.string("type"))
                .unwrap_or("html")
                .to_owned();
            if !matches!(response_type.as_str(), "html" | "json") {
                return Err(format!(
                    "{prefix}.response.type: unsupported {response_type}"
                ));
            }
            // Upstream ignores the setting outside JSON paths; reject rather than ignore.
            if empty_no_results && response_type != "json" {
                return Err(format!(
                    "{prefix}.response.noResultsMessage: only supported for JSON responses"
                ));
            }
            let categories = node
                .get("categories")
                .map(|n| {
                    n.items()
                        .ok_or_else(|| format!("{prefix}.categories: expected sequence"))
                })
                .transpose()?
                .unwrap_or_default()
                .iter()
                .map(|n| scalar(n, &prefix).map(str::to_owned))
                .collect::<Result<Vec<_>, _>>()?;
            paths.push(PathPlan {
                path: Template::compile(
                    node.string("path")
                        .ok_or_else(|| format!("{prefix}.path: missing"))?,
                )?,
                inputs: fields,
                categories,
                method,
                response_type,
                empty_no_results,
            });
        }
        let mut headers = Vec::new();
        if let Some(node) = search.get("headers") {
            let pairs = node.pairs().ok_or("$.search.headers: expected mapping")?;
            if pairs.len() > MAX_INPUTS {
                return Err("$.search.headers: too many headers".into());
            }
            for (key, values) in pairs {
                let key = scalar(key, "$.search.headers")?;
                if !key
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c))
                    || key.is_empty()
                {
                    return Err("$.search.headers: invalid header name".into());
                }
                let values = values
                    .items()
                    .ok_or("$.search.headers: expected sequence")?;
                // Upstream ParseCustomHeaders intentionally uses only the first value.
                let value = values.first().ok_or("$.search.headers: empty values")?;
                headers.push((
                    key.to_owned(),
                    Template::compile(scalar(value, "$.search.headers")?)?,
                ));
            }
        }
        Ok(Self {
            base,
            charset,
            paths,
            headers,
            allow_empty: boolean(
                search.get("allowEmptyInputs"),
                false,
                "$.search.allowEmptyInputs",
            )?,
        })
    }
    pub fn charset(&self) -> Charset {
        self.charset
    }
    pub fn response_types(&self) -> impl Iterator<Item = &str> {
        self.paths.iter().map(|path| path.response_type.as_str())
    }
    pub fn has_empty_no_results(&self) -> bool {
        self.paths.iter().any(|path| path.empty_no_results)
    }
    pub fn build(&self, ctx: &Context) -> Result<Vec<BuiltRequest>, String> {
        let mut requests = Vec::new();
        let mut seen = BTreeSet::new();
        for path in &self.paths {
            let mut local = ctx.clone();
            if !path.categories.is_empty() {
                let mapped = ctx
                    .values
                    .get(".Categories")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let selected: Vec<_> = mapped
                    .into_iter()
                    .filter(|v| {
                        v.as_str()
                            .is_some_and(|s| path.categories.iter().any(|c| c == s))
                    })
                    .collect();
                let negated = path.categories.first().is_some_and(|s| s == "!");
                if selected.is_empty() != negated {
                    continue;
                }
                // This follows upstream, including empty intersection for a negated path.
                local.values.insert(".Categories".into(), selected.into());
            }
            let rendered = path.path.render_path(&local)?;
            if rendered.len() > MAX_REQUEST_BYTES || rendered.chars().any(char::is_control) {
                return Err("$.search.path: invalid or oversized path".into());
            }
            let mut target = self
                .base
                .join(&rendered)
                .map_err(|e| format!("$.search.path: {e}"))?;
            if !matches!(target.scheme(), "http" | "https")
                || target.host_str().is_none()
                || !target.username().is_empty()
                || target.password().is_some()
            {
                return Err("$.search.path: expected HTTP(S) URL without credentials".into());
            }
            let mut pairs = Vec::new();
            let mut total = 0usize;
            for (key, template, source) in &path.inputs {
                let value = if key == "$raw" {
                    template.render_raw(&local)
                } else {
                    template.render(&local)
                }
                .map_err(|e| format!("{source}: {e}"))?;
                total = total.saturating_add(key.len()).saturating_add(value.len());
                if total > MAX_REQUEST_BYTES {
                    return Err(format!("{source}: request byte budget exceeded"));
                }
                if key == "$raw" {
                    for part in value.split('&') {
                        let (key, value) = part.split_once('=').unwrap_or((part, ""));
                        if key.is_empty() {
                            continue;
                        }
                        // StringUtil.GetQueryString leaves keys literal. Restrict only bytes
                        // that would change URL structure or inject transport controls.
                        if key
                            .chars()
                            .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '#' | '?'))
                        {
                            return Err(format!("{source}: unsafe raw input key"));
                        }
                        pairs.push((key.to_owned(), value.to_owned()));
                        if pairs.len() > MAX_INPUTS {
                            return Err(format!("{source}: expanded input count limit exceeded"));
                        }
                    }
                } else if self.allow_empty || !value.trim().is_empty() {
                    pairs.push((key.clone(), value));
                }
                if pairs.len() > MAX_INPUTS {
                    return Err(format!("{source}: expanded input count limit exceeded"));
                }
            }
            let mut encoded = String::new();
            for (key, value) in pairs {
                // Raw substitutions were already UTF-8 encoded by applyGoTemplateText.
                // The final GetQueryString(Encoding) intentionally encodes their percent
                // signs again; literal values use the definition encoding.
                let value = if path.method == "POST" {
                    form_encode(&value)
                } else {
                    self.charset
                        .url_encode(&value)
                        .map_err(|e| format!("$.search.inputs: {e}"))?
                };
                let key = if path.method == "POST" {
                    form_encode(&key)
                } else {
                    key
                };
                if encoded
                    .len()
                    .saturating_add(key.len())
                    .saturating_add(value.len())
                    .saturating_add(2)
                    > MAX_REQUEST_BYTES
                {
                    return Err("$.search.inputs: encoded request byte budget exceeded".into());
                }
                if !encoded.is_empty() {
                    encoded.push('&');
                }
                encoded.push_str(&key);
                encoded.push('=');
                encoded.push_str(&value);
            }
            let body = if path.method == "POST" {
                Some(encoded)
            } else {
                if !encoded.is_empty() {
                    // Upstream appends '?' even when a literal path already contains a query.
                    let text = format!("{target}?{encoded}");
                    target = Url::parse(&text).map_err(|e| format!("$.search.inputs: {e}"))?;
                }
                None
            };
            if path.method == "GET" && !seen.insert(target.to_string()) {
                continue;
            }
            let mut headers = Vec::new();
            let mut header_bytes = 0usize;
            for (name, template) in &self.headers {
                let value = template.render(&local)?;
                header_bytes = header_bytes
                    .saturating_add(name.len())
                    .saturating_add(value.len());
                if header_bytes > MAX_REQUEST_BYTES
                    || value.chars().any(char::is_control)
                    || value.len() > 8192
                {
                    return Err(format!(
                        "$.search.headers.{name}: invalid or oversized value"
                    ));
                }
                headers.push((name.clone(), value));
            }
            if target
                .as_str()
                .len()
                .saturating_add(body.as_ref().map_or(0, String::len))
                .saturating_add(header_bytes)
                > MAX_REQUEST_BYTES
            {
                return Err("$.search.path: request byte budget exceeded".into());
            }
            requests.push(BuiltRequest {
                url: target.into(),
                method: path.method.clone(),
                headers,
                body,
                response_type: path.response_type.clone(),
                empty_no_results: path.empty_no_results,
            });
        }
        Ok(requests)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cardigann::{parse, Limits};
    fn context() -> Context {
        Context {
            values: [
                (".Keywords".into(), "a/b & c".into()),
                (".Categories".into(), serde_json::json!(["1", "2"])),
            ]
            .into(),
            now: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        }
    }
    fn plan(search: &str) -> Result<RequestPlan, String> {
        let root = parse(
            &format!("links: [https://example.test/]\nsearch:\n{search}"),
            Limits::default(),
        )
        .unwrap();
        RequestPlan::compile(&root)
    }
    fn encoded_plan(encoding: &str, search: &str) -> Result<RequestPlan, String> {
        let root = parse(
            &format!("encoding: {encoding}\nlinks: [https://example.test/]\nsearch:\n{search}"),
            Limits::default(),
        )
        .unwrap();
        RequestPlan::compile(&root)
    }
    fn keywords(keywords: &str) -> Context {
        let mut ctx = context();
        ctx.values.insert(".Keywords".into(), keywords.into());
        ctx
    }
    #[test]
    fn latin1_labels_are_exact_and_other_labels_keep_errors() {
        for label in ["iso-8859-1", "ISO-8859-1", "Iso-8859-1"] {
            assert_eq!(
                encoded_plan(label, "  path: search\n").unwrap().charset(),
                Charset::Latin1
            );
        }
        for label in ["UTF-8", "utf8", "Utf-8"] {
            assert_eq!(
                encoded_plan(label, "  path: search\n").unwrap().charset(),
                Charset::Utf8
            );
        }
        assert_eq!(plan("  path: search\n").unwrap().charset(), Charset::Utf8);
        for label in [
            "latin1",
            "ISO_8859-1",
            "iso-8859-15",
            "windows-1252",
            "iso8859-1",
        ] {
            assert_eq!(
                encoded_plan(label, "  path: search\n").unwrap_err(),
                format!("$.encoding: unsupported encoding {label}")
            );
        }
        for label in ["[iso-8859-1]", "{name: iso-8859-1}"] {
            assert!(encoded_plan(label, "  path: search\n")
                .unwrap_err()
                .contains("$.encoding: unsupported non-scalar"));
        }
    }
    #[test]
    fn latin1_get_values_encode_exact_bytes() {
        let p = encoded_plan(
            "iso-8859-1",
            "  path: search\n  inputs: {q: '{{ .Keywords }}'}\n",
        )
        .unwrap();
        let utf8 = plan("  path: search\n  inputs: {q: '{{ .Keywords }}'}\n").unwrap();
        let url = |p: &RequestPlan, k: &str| p.build(&keywords(k)).map(|r| r[0].url.clone());
        assert_eq!(
            url(&p, "café").unwrap(),
            "https://example.test/search?q=caf%E9"
        );
        assert_eq!(
            url(&utf8, "café").unwrap(),
            "https://example.test/search?q=caf%C3%A9"
        );
        assert_eq!(
            url(&p, "a b&c=d'~!*()").unwrap(),
            "https://example.test/search?q=a+b%26c%3Dd%27%7E!*()"
        );
        assert_eq!(
            url(&p, "\u{80}ÿ").unwrap(),
            "https://example.test/search?q=%80%FF"
        );
        // No .NET '?' replacement or best-fit guess; the build fails before any transport.
        for value in ["€", "Ā", "日本", "😀", "café€"] {
            assert_eq!(
                url(&p, value).unwrap_err(),
                "$.search.inputs: value contains text not representable in ISO-8859-1"
            );
        }
    }
    #[test]
    fn latin1_literals_raw_keys_paths_and_headers() {
        let p = encoded_plan(
            "iso-8859-1",
            "  path: 'é/{{ .Keywords }}'\n  headers: {X-Q: ['{{ .Keywords }}é']}\n  inputs:\n    literal: é\n    $raw: 'x={{ .Keywords }}&y=é&é=1'\n",
        )
        .unwrap();
        let built = p.build(&keywords("é")).unwrap();
        assert_eq!(
            built[0].url,
            "https://example.test/%C3%A9/%C3%A9?literal=%E9&x=%25C3%25A9&y=%E9&%C3%A9=1"
        );
        assert_eq!(built[0].headers, [("X-Q".into(), "éé".into())]);
        let utf8 = plan("  path: 'é/{{ .Keywords }}'\n  inputs:\n    literal: é\n    $raw: 'x={{ .Keywords }}&y=é&é=1'\n").unwrap();
        assert_eq!(
            utf8.build(&keywords("é")).unwrap()[0].url,
            "https://example.test/%C3%A9/%C3%A9?literal=%C3%A9&x=%25C3%25A9&y=%C3%A9&%C3%A9=1"
        );
        // Raw substitutions are always UTF-8 encoded upstream, so they cannot fail here.
        let raw = encoded_plan(
            "iso-8859-1",
            "  path: search\n  inputs: {$raw: 'x={{ .Keywords }}'}\n",
        )
        .unwrap();
        assert_eq!(
            raw.build(&keywords("€")).unwrap()[0].url,
            "https://example.test/search?x=%25E2%2582%25AC"
        );
        let raw = encoded_plan("iso-8859-1", "  path: search\n  inputs: {$raw: 'x=€'}\n").unwrap();
        assert!(raw.build(&context()).unwrap_err().contains("ISO-8859-1"));
    }
    #[test]
    fn latin1_post_is_a_compile_error() {
        assert_eq!(
            encoded_plan("iso-8859-1", "  path: search\n  method: post\n").unwrap_err(),
            "$.search.method: unsupported POST with non-UTF-8 encoding"
        );
        assert_eq!(
            encoded_plan(
                "ISO-8859-1",
                "  paths:\n    - {path: a}\n    - {path: b, method: POST}\n"
            )
            .unwrap_err(),
            "$.search.paths[1].method: unsupported POST with non-UTF-8 encoding"
        );
        assert!(encoded_plan("utf-8", "  path: search\n  method: post\n").is_ok());
    }
    #[test]
    fn repeated_inputs_inheritance_headers_and_post() {
        let p = plan("  inputs:\n    q: '{{ .Keywords }}'\n    cat: first\n    cat: second\n  headers:\n    X-Query: ['{{ .Keywords }}', ignored]\n  paths:\n    - path: search\n      inputs: {cat: third}\n    - path: submit\n      method: post\n      inheritinputs: false\n      inputs: {q: '{{ .Keywords }}'}\n").unwrap();
        let built = p.build(&context()).unwrap();
        assert_eq!(built.len(), 2);
        assert_eq!(
            built[0].url,
            "https://example.test/search?q=a%2Fb+%26+c&cat=first&cat=second&cat=third"
        );
        assert_eq!(built[0].headers, [("X-Query".into(), "a/b & c".into())]);
        assert_eq!(built[1].body.as_deref(), Some("q=a%2Fb+%26+c"));
        assert_eq!(built[1].method, "POST");
    }
    #[test]
    fn categories_dedup_and_empty_inputs() {
        let p = plan("  inputs: {empty: '', blank: '  '}\n  paths:\n    - {path: search, categories: ['1']}\n    - {path: search, categories: ['1']}\n    - {path: absent, categories: ['3']}\n    - {path: excluded, categories: ['!', '1']}\n    - {path: included, categories: ['!', '3']}\n").unwrap();
        let built = p.build(&context()).unwrap();
        assert_eq!(
            built.iter().map(|r| r.url.as_str()).collect::<Vec<_>>(),
            [
                "https://example.test/search",
                "https://example.test/included"
            ]
        );
        let p = plan("  path: search\n  allowEmptyInputs: true\n  inputs: {empty: ''}\n").unwrap();
        assert_eq!(
            p.build(&context()).unwrap()[0].url,
            "https://example.test/search?empty="
        );
    }
    #[test]
    fn path_substitutions_are_encoded_without_encoding_literals() {
        let p = plan("  path: 'search/{{ .Keywords }}'\n").unwrap();
        assert_eq!(
            p.build(&context()).unwrap()[0].url,
            "https://example.test/search/a%2Fb%20%26%20c"
        );
    }
    #[test]
    fn upstream_value_encoding_and_literal_safe_keys() {
        let p = plan("  path: search\n  inputs: { 'cat[]': '{{ .Keywords }}' }\n").unwrap();
        let mut ctx = context();
        ctx.values.insert(".Keywords".into(), "!()* ~&='é".into());
        assert_eq!(
            p.build(&ctx).unwrap()[0].url,
            "https://example.test/search?cat[]=!()*+%7E%26%3D%27%C3%A9"
        );
        for key in ["a&b", "a=b", "a#b", "a%b", "a+b", "a b"] {
            assert!(plan(&format!("  path: search\n  inputs: {{ '{key}': value }}\n")).is_err());
        }
    }
    #[test]
    fn raw_order_double_encoding_post_and_source_paths() {
        let p = plan("  inputs:\n    before: one\n    $raw: 'x={{ .Keywords }}&x=&bare&&=skip&cat[]=a=b&$key=100%&{{ range .Categories }}cat{{.}}=1&{{end}}'\n    after: two\n  paths:\n    - path: 'search?literal=1'\n      inputs: {$raw: 'tail={{ if .Keywords }}yes{{else}}no{{end}}'}\n    - {path: submit, method: post}\n").unwrap();
        let mut ctx = context();
        ctx.values.insert(".Keywords".into(), "é &=+%".into());
        let requests = p.build(&ctx).unwrap();
        assert_eq!(requests[0].url, "https://example.test/search?literal=1?before=one&x=%25C3%25A9%2B%2526%253D%252B%2525&x=&bare=&cat[]=a%3Db&$key=100%25&cat1=1&cat2=1&after=two&tail=yes");
        assert_eq!(requests[1].body.as_deref(), Some("before=one&x=%25C3%25A9%2B%2526%253D%252B%2525&x=&bare=&cat%5B%5D=a%3Db&%24key=100%25&cat1=1&cat2=1&after=two"));
        let p = plan("  paths: [{path: search, inputs: {$raw: '{{ .Missing }}'}}]\n").unwrap();
        assert!(p
            .build(&ctx)
            .unwrap_err()
            .contains("$.search.paths[0].inputs.$raw"));
        let p = plan("  path: search\n  inputs: {$raw: '{{ .Keywords }}'}\n").unwrap();
        ctx.values
            .insert(".Keywords".into(), "a=1&".repeat(257).into());
        // A substitution is encoded, so it cannot manufacture literal separators.
        assert_eq!(p.build(&ctx).unwrap().len(), 1);
        assert!(plan(&format!(
            "  path: search\n  inputs: {{$raw: '{}'}}\n",
            "x=&".repeat(257)
        ))
        .unwrap()
        .build(&ctx)
        .unwrap_err()
        .contains("count limit"));
        for raw in ["x#fragment=v", "x?query=v", "bad key=v"] {
            assert!(
                plan(&format!("  path: search\n  inputs: {{$raw: '{raw}'}}\n"))
                    .unwrap()
                    .build(&ctx)
                    .is_err()
            );
        }
        let p = plan("  path: search\n  inputs: {$raw: '{{ .Keywords }}'}\n").unwrap();
        ctx.values
            .insert(".Keywords".into(), "é".repeat(40_000).into());
        assert!(p.build(&ctx).unwrap_err().contains("budget"));
        let p = plan("  path: search\n  inputs: {'cat[]': '!()*~'}\n  method: post\n").unwrap();
        assert_eq!(
            p.build(&context()).unwrap()[0].body.as_deref(),
            Some("cat%5B%5D=%21%28%29%2A~")
        );
    }
    #[test]
    fn combined_request_budget_and_raw_injection_boundaries() {
        let p = plan("  path: '{{ .Keywords }}'\n  method: post\n  inputs: {a: '{{ .Keywords }}', b: '{{ .Keywords }}'}\n").unwrap();
        let mut ctx = context();
        ctx.values
            .insert(".Keywords".into(), "a".repeat(50_000).into());
        assert!(p.build(&ctx).unwrap_err().contains("budget"));
        for path in ["ftp://example.test/x", "https://user@example.test/x"] {
            assert!(plan(&format!("  path: {path:?}\n"))
                .unwrap()
                .build(&context())
                .is_err());
        }
    }
    #[test]
    fn unsupported_and_header_injection_are_errors() {
        assert!(
            plan("  path: search\n  inputs: {$raw: '{{ unknown .Keywords }}'}\n")
                .unwrap_err()
                .contains("$raw")
        );
        assert!(plan("  paths: [{path: x, queryseparator: ';'}]\n").is_err());
        assert!(plan("  paths: [{path: x, unknown: true}]\n").is_err());
        assert!(
            plan("  paths: [{path: x, response: {type: json, noResultsMessage: empty}}]\n")
                .is_err()
        );
        // Below schema validation, a non-JSON path rejects rather than ignores the policy.
        for response in [
            "{type: html, noResultsMessage: ''}",
            "{noResultsMessage: ''}",
        ] {
            assert!(
                plan(&format!("  paths: [{{path: x, response: {response}}}]\n"))
                    .unwrap_err()
                    .contains("only supported for JSON")
            );
        }
        let p = plan("  paths: [{path: x, response: {type: json, noResultsMessage: ''}}, {path: y, response: {type: json}}]\n").unwrap();
        assert!(p.has_empty_no_results());
        let built = p.build(&context()).unwrap();
        assert!(built[0].empty_no_results() && !built[1].empty_no_results());
        let p = plan("  path: search\n  headers: {X-Test: ['{{ .Keywords }}']}\n").unwrap();
        let mut ctx = context();
        ctx.values
            .insert(".Keywords".into(), "hello\r\nInjected: value".into());
        assert!(p.build(&ctx).is_err());
    }
    fn root(source: &str) -> Result<RequestPlan, String> {
        RequestPlan::compile(&parse(source, Limits::default()).expect("parse keeps repeated keys"))
    }
    const AMBIGUOUS: &str = "repeated structural property with noResultsMessage";
    #[test]
    fn policy_definitions_reject_repeated_structural_keys_at_the_request_layer() {
        let policy = "{type: json, noResultsMessage: ''}";
        // Repeated response blocks, either order, against each alternative.
        for other in [
            "{type: json}",
            "{type: json, noResultsMessage: NoTorrents}",
            "{type: html}",
            policy,
        ] {
            for (first, second) in [(policy, other), (other, policy)] {
                let e = plan(&format!(
                    "  paths:\n    - path: api\n      response: {first}\n      response: {second}\n"
                ))
                .unwrap_err();
                assert_eq!(e, format!("$.search.paths[0]: {AMBIGUOUS}"));
            }
        }
        // Repeated keys inside the response block and on the path entry.
        for response in [
            "{type: json, noResultsMessage: '', noResultsMessage: ''}",
            "{type: json, noResultsMessage: '', noResultsMessage: x}",
            "{type: json, type: json, noResultsMessage: ''}",
            "{noResultsMessage: '', type: json, type: html}",
        ] {
            let e = plan(&format!(
                "  paths:\n    - {{path: api, response: {response}}}\n"
            ))
            .unwrap_err();
            assert_eq!(e, format!("$.search.paths[0].response: {AMBIGUOUS}"));
        }
        let e = plan(&format!(
            "  paths:\n    - {{path: x}}\n    - {{path: api, path: other, response: {policy}}}\n"
        ))
        .unwrap_err();
        assert_eq!(e, format!("$.search.paths[1]: {AMBIGUOUS}"));
        // Repeated path lists and search blocks, policy in the first or the second.
        let lists = [
            format!("  paths: [{{path: api, response: {policy}}}]\n"),
            "  paths: [{path: api, response: {type: json}}]\n".to_owned(),
        ];
        for (first, second) in [(&lists[0], &lists[1]), (&lists[1], &lists[0])] {
            assert_eq!(
                plan(&format!("{first}{second}")).unwrap_err(),
                format!("$.search: {AMBIGUOUS}")
            );
            let source =
                format!("links: [https://example.test/]\nsearch:\n{first}search:\n{second}");
            assert_eq!(root(&source).unwrap_err(), format!("$: {AMBIGUOUS}"));
        }
        let source = format!(
            "links: [https://example.test/]\nlinks: [https://other.test/]\nsearch:\n{}",
            lists[0]
        );
        assert_eq!(root(&source).unwrap_err(), format!("$: {AMBIGUOUS}"));
        // The same structures without a policy keep their existing first-match behavior.
        let ordinary = "{type: json}";
        let p = plan(&format!(
            "  paths:\n    - path: api\n      response: {ordinary}\n      response: {{type: html}}\n"
        ))
        .unwrap();
        assert_eq!(p.response_types().collect::<Vec<_>>(), ["json"]);
        assert!(plan(&format!("{}{}", lists[1], lists[1])).is_ok());
        let source = format!(
            "links: [https://example.test/]\nsearch:\n{}search:\n{}",
            lists[1], lists[1]
        );
        assert!(root(&source).is_ok());
    }
    #[test]
    fn policy_definitions_keep_repeated_input_keys() {
        let p = plan(
            "  inputs: {a: 1, a: 2}\n  paths:\n    - {path: api, inputs: {b: 3, b: 4}, response: {type: json, noResultsMessage: ''}}\n",
        )
        .unwrap();
        let built = p.build(&context()).unwrap();
        assert!(built[0].empty_no_results());
        assert!(
            built[0].url.ends_with("/api?a=1&a=2&b=3&b=4"),
            "{}",
            built[0].url
        );
    }
}
