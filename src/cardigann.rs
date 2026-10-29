//! Offline, bounded Cardigann document model. No tracker execution is implemented.
use serde::Serialize;
use std::{collections::BTreeSet, fs::File, io::Read, path::Path};
use yaml_rust2::{
    parser::{Event, Parser},
    scanner::{Marker, TScalarStyle},
};

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct Location {
    pub line: usize,
    pub column: usize,
}
impl From<Marker> for Location {
    fn from(m: Marker) -> Self {
        Self {
            line: m.line(),
            column: m.col(),
        }
    }
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Node {
    pub location: Location,
    pub value: Value,
}
/// Every mapping is a sequence of pairs, including unknown mappings and repeated keys.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Value {
    Scalar(Scalar),
    Sequence(Vec<Node>),
    Mapping(Vec<(Node, Node)>),
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Scalar {
    pub text: String,
    pub style: ScalarStyle,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScalarStyle {
    Plain,
    SingleQuoted,
    DoubleQuoted,
    Literal,
    Folded,
}
impl Node {
    pub fn text(&self) -> Option<&str> {
        if let Value::Scalar(s) = &self.value {
            Some(&s.text)
        } else {
            None
        }
    }
    pub fn pairs(&self) -> Option<&[(Node, Node)]> {
        if let Value::Mapping(v) = &self.value {
            Some(v)
        } else {
            None
        }
    }
    pub fn items(&self) -> Option<&[Node]> {
        if let Value::Sequence(v) = &self.value {
            Some(v)
        } else {
            None
        }
    }
    /// First match only; use `all` or `pairs` for ordered execution fields.
    pub fn get(&self, key: &str) -> Option<&Node> {
        self.pairs()?
            .iter()
            .find_map(|(k, v)| (k.text() == Some(key)).then_some(v))
    }
    pub fn all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a Node> {
        self.pairs()
            .unwrap_or_default()
            .iter()
            .filter_map(move |(k, v)| (k.text() == Some(key)).then_some(v))
    }
    pub fn string(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(Node::text)
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub bytes: usize,
    pub depth: usize,
    pub nodes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            bytes: 2 * 1024 * 1024,
            depth: 64,
            nodes: 100_000,
        }
    }
}
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
}
#[derive(Clone, Debug, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub path: String,
    pub location: Option<Location>,
    pub message: String,
}
fn diagnostic(code: &str, message: &str, location: Option<Location>) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        code: code.into(),
        path: "$".into(),
        location,
        message: message.into(),
    }
}
struct Reader<'a> {
    parser: Parser<std::str::Chars<'a>>,
    limits: Limits,
    nodes: usize,
}
impl Reader<'_> {
    fn next(&mut self) -> Result<(Event, Marker), Diagnostic> {
        self.parser
            .next_token()
            .map_err(|e| diagnostic("yaml_syntax", &e.to_string(), Some((*e.marker()).into())))
    }
    fn node(&mut self, event: Event, mark: Marker, depth: usize) -> Result<Node, Diagnostic> {
        let loc = Some(mark.into());
        if depth > self.limits.depth {
            return Err(diagnostic(
                "depth_limit",
                "Document nesting limit exceeded",
                loc,
            ));
        }
        self.nodes += 1;
        if self.nodes > self.limits.nodes {
            return Err(diagnostic(
                "node_limit",
                "Document node limit exceeded",
                loc,
            ));
        }
        let value = match event {
            Event::Scalar(text, style, anchor, tag) => {
                if anchor != 0 || tag.is_some() {
                    return Err(diagnostic("yaml_anchor_or_tag", "Anchors and explicit tags are unsupported; no alias expansion is performed", loc));
                }
                Value::Scalar(Scalar {
                    text,
                    style: match style {
                        TScalarStyle::SingleQuoted => ScalarStyle::SingleQuoted,
                        TScalarStyle::DoubleQuoted => ScalarStyle::DoubleQuoted,
                        TScalarStyle::Literal => ScalarStyle::Literal,
                        TScalarStyle::Folded => ScalarStyle::Folded,
                        _ => ScalarStyle::Plain,
                    },
                })
            }
            Event::SequenceStart(anchor, ref tag) | Event::MappingStart(anchor, ref tag) => {
                if anchor != 0 || tag.is_some() {
                    return Err(diagnostic("yaml_anchor_or_tag", "Anchors and explicit tags are unsupported; no alias expansion is performed", loc));
                }
                let mapping = matches!(event, Event::MappingStart(..));
                let mut sequence = Vec::new();
                let mut pairs = Vec::new();
                loop {
                    let (e, m) = self.next()?;
                    if (mapping && e == Event::MappingEnd) || (!mapping && e == Event::SequenceEnd)
                    {
                        break;
                    }
                    let key = self.node(e, m, depth + 1)?;
                    if mapping {
                        if key.text().is_none() {
                            return Err(diagnostic(
                                "complex_key",
                                "Only scalar mapping keys are supported",
                                Some(key.location),
                            ));
                        }
                        if key.text() == Some("<<") {
                            return Err(diagnostic(
                                "yaml_merge",
                                "YAML merge keys are unsupported",
                                Some(key.location),
                            ));
                        }
                        let (e, m) = self.next()?;
                        pairs.push((key, self.node(e, m, depth + 1)?));
                    } else {
                        sequence.push(key);
                    }
                }
                if mapping {
                    Value::Mapping(pairs)
                } else {
                    Value::Sequence(sequence)
                }
            }
            Event::Alias(_) => {
                return Err(diagnostic(
                    "yaml_alias",
                    "Aliases are unsupported; no alias expansion is performed",
                    loc,
                ))
            }
            _ => {
                return Err(diagnostic(
                    "yaml_structure",
                    "Expected a scalar, sequence or mapping",
                    loc,
                ))
            }
        };
        Ok(Node {
            location: mark.into(),
            value,
        })
    }
}
/// Parse exactly one UTF-8 YAML document. Scalars are decoded strings, never implicitly typed.
pub fn parse(source: &str, limits: Limits) -> Result<Node, Diagnostic> {
    if source.len() > limits.bytes {
        return Err(diagnostic(
            "byte_limit",
            "Document byte limit exceeded",
            None,
        ));
    }
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    if limits.depth > 128 {
        return Err(diagnostic(
            "limit_configuration",
            "Maximum configurable nesting depth is 128",
            None,
        ));
    }
    // Directives alter YAML semantics outside the supported, deliberately narrow dialect.
    if source.lines().any(|l| l.starts_with('%')) {
        return Err(diagnostic(
            "yaml_directive",
            "YAML directives are unsupported",
            None,
        ));
    }
    let mut r = Reader {
        parser: Parser::new_from_str(source),
        limits,
        nodes: 0,
    };
    if r.next()?.0 != Event::StreamStart || r.next()?.0 != Event::DocumentStart {
        return Err(diagnostic(
            "yaml_document",
            "Expected one YAML document",
            None,
        ));
    }
    let (e, m) = r.next()?;
    let root = r.node(e, m, 1)?;
    if r.next()?.0 != Event::DocumentEnd || r.next()?.0 != Event::StreamEnd {
        return Err(diagnostic(
            "yaml_documents",
            "Exactly one YAML document is allowed",
            None,
        ));
    }
    Ok(root)
}
/// Bounded read also protects against files growing after metadata inspection.
pub fn load(path: &Path, limits: Limits) -> Result<Node, Diagnostic> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|f| {
            f.take(limits.bytes.saturating_add(1) as u64)
                .read_to_end(&mut bytes)
        })
        .map_err(|e| diagnostic("io", &e.to_string(), None))?;
    if bytes.len() > limits.bytes {
        return Err(diagnostic(
            "byte_limit",
            "Document byte limit exceeded",
            None,
        ));
    }
    let source = std::str::from_utf8(&bytes)
        .map_err(|_| diagnostic("encoding", "Definition must be UTF-8", None))?;
    parse(source, limits)
}

/// Typed, borrowing views retain access to the full raw node. No defaults mutate the AST.
#[derive(Clone, Copy, Debug)]
pub struct Definition<'a> {
    pub raw: &'a Node,
}
impl<'a> Definition<'a> {
    pub fn metadata(self) -> Metadata<'a> {
        Metadata { raw: self.raw }
    }
    pub fn settings(self) -> impl Iterator<Item = Setting<'a>> {
        self.raw
            .get("settings")
            .and_then(Node::items)
            .unwrap_or_default()
            .iter()
            .map(|raw| Setting { raw })
    }
    pub fn caps(self) -> Option<Capabilities<'a>> {
        self.raw.get("caps").map(|raw| Capabilities { raw })
    }
    pub fn login(self) -> Option<Login<'a>> {
        self.raw.get("login").map(|raw| Login { raw })
    }
    pub fn search(self) -> Option<Search<'a>> {
        self.raw.get("search").map(|raw| Search { raw })
    }
    pub fn download(self) -> Option<Download<'a>> {
        self.raw.get("download").map(|raw| Download { raw })
    }
}
macro_rules! view { ($name:ident, $($method:ident => $key:literal),* $(,)?) => {
    #[derive(Clone, Copy, Debug)] pub struct $name<'a> { pub raw: &'a Node }
    impl $name<'_> { $(pub fn $method(&self) -> Option<&str> { self.raw.string($key) })* }
}; }
view!(Metadata, id => "id", name => "name", description => "description", language => "language", access_type => "type", encoding => "encoding");
view!(Setting, name => "name", kind => "type", label => "label", default => "default");
view!(Capabilities,);
view!(Login, method => "method", path => "path", form => "form", submit_path => "submitpath");
view!(Search, path => "path");
view!(Download, method => "method");
view!(Selector, selector => "selector", text => "text", attribute => "attribute", default => "default");
impl Metadata<'_> {
    pub fn follow_redirect(&self) -> bool {
        self.raw.string("followredirect") == Some("true")
    }
    pub fn test_link_torrent(&self) -> bool {
        self.raw.string("testlinktorrent") != Some("false")
    }
    pub fn request_delay(&self) -> Option<f64> {
        self.raw.string("requestDelay").and_then(|v| v.parse().ok())
    }
}
impl<'a> Capabilities<'a> {
    pub fn modes(&self) -> &'a [(Node, Node)] {
        self.raw
            .get("modes")
            .and_then(Node::pairs)
            .unwrap_or_default()
    }
    pub fn categories(&self) -> &'a [(Node, Node)] {
        self.raw
            .get("categories")
            .and_then(Node::pairs)
            .unwrap_or_default()
    }
    pub fn category_mappings(&self) -> &'a [Node] {
        self.raw
            .get("categorymappings")
            .and_then(Node::items)
            .unwrap_or_default()
    }
}
#[derive(Debug)]
pub struct Field<'a> {
    pub name: &'a str,
    pub selector: Selector<'a>,
}
impl Field<'_> {
    pub fn base_name(&self) -> &str {
        self.name.split('|').next().unwrap_or("")
    }
    pub fn modifiers(&self) -> impl Iterator<Item = &str> {
        self.name.split('|').skip(1)
    }
    pub fn intermediate(&self) -> bool {
        self.base_name().starts_with('_')
    }
}
impl<'a> Search<'a> {
    pub fn fields(&self) -> impl Iterator<Item = Field<'a>> {
        self.raw
            .get("fields")
            .and_then(Node::pairs)
            .unwrap_or_default()
            .iter()
            .filter_map(|(k, v)| {
                k.text().map(|name| Field {
                    name,
                    selector: Selector { raw: v },
                })
            })
    }
    pub fn paths(&self) -> &'a [Node] {
        self.raw
            .get("paths")
            .and_then(Node::items)
            .unwrap_or_default()
    }
    pub fn rows(&self) -> Option<Selector<'a>> {
        self.raw.get("rows").map(|raw| Selector { raw })
    }
}

fn schema() -> &'static serde_json::Value {
    static SCHEMA: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    SCHEMA.get_or_init(|| {
        serde_json::from_str(include_str!("../vendor/jackett/schema.json"))
            .expect("pinned schema is valid JSON")
    })
}
fn resolve(s: &serde_json::Value) -> &serde_json::Value {
    s.get("$ref")
        .and_then(|v| v.as_str())
        .and_then(|r| r.strip_prefix('#'))
        .and_then(|p| schema().pointer(p))
        .unwrap_or(s)
}
fn shape(node: &Node, kind: &str) -> bool {
    match kind {
        "object" => node.pairs().is_some(),
        "array" => node.items().is_some(),
        "string" => node.text().is_some(),
        "boolean" => matches!(node.text(), Some("true" | "false")),
        "integer" => node.text().is_some_and(|v| v.parse::<i64>().is_ok()),
        "number" => node
            .text()
            .is_some_and(|v| v.parse::<f64>().is_ok_and(f64::is_finite)),
        _ => false,
    }
}
fn issue(
    out: &mut Vec<Diagnostic>,
    node: &Node,
    path: &str,
    code: &str,
    message: &str,
    severity: Severity,
) {
    out.push(Diagnostic {
        severity,
        code: code.into(),
        path: path.into(),
        location: Some(node.location),
        message: message.into(),
    });
}
fn validate_node(node: &Node, s: &serde_json::Value, path: &str, out: &mut Vec<Diagnostic>) {
    let s = resolve(s);
    if let Some(kind) = s.get("type").and_then(|v| v.as_str()) {
        if !shape(node, kind) {
            issue(
                out,
                node,
                path,
                "model_type",
                &format!("Expected {kind}"),
                Severity::Error,
            );
            return;
        }
    }
    if let Some(choices) = s.get("enum").and_then(|v| v.as_array()) {
        if !choices.iter().any(|v| v.as_str() == node.text()) {
            issue(
                out,
                node,
                path,
                "model_enum",
                "Value is outside pinned schema enumeration",
                Severity::Error,
            );
        }
    }
    if let Some(alternatives) = s.get("oneOf").and_then(|v| v.as_array()) {
        // Scalar/sequence unions are checked as shapes, without implicitly resolving scalar types.
        let types: Vec<_> = alternatives
            .iter()
            .filter_map(|v| v.get("type").and_then(|v| v.as_str()))
            .collect();
        if !types.is_empty() && !types.iter().any(|t| shape(node, t)) {
            issue(
                out,
                node,
                path,
                "model_union",
                "Value has no supported schema shape",
                Severity::Error,
            );
            return;
        }
    }
    if let Some(alternatives) = s.get("oneOf").and_then(|v| v.as_array()) {
        // All scalars stay textual, so primitive scalar alternatives can overlap. Only
        // descend unambiguous container branches to validate their nested item shapes.
        if let Some(branch) = alternatives.iter().find(|v| {
            v.get("type")
                .and_then(|v| v.as_str())
                .is_some_and(|t| matches!(t, "array" | "object") && shape(node, t))
        }) {
            validate_node(node, branch, path, out);
        }
    }
    for (combinator, exactly_one) in [("oneOf", true), ("anyOf", false)] {
        if let Some(alternatives) = s.get(combinator).and_then(|v| v.as_array()) {
            let required_sets: Vec<_> = alternatives
                .iter()
                .filter_map(|a| a.get("required").and_then(|v| v.as_array()))
                .collect();
            if required_sets.len() == alternatives.len() {
                let matches = required_sets
                    .iter()
                    .filter(|keys| {
                        keys.iter()
                            .filter_map(|k| k.as_str())
                            .all(|k| node.get(k).is_some())
                    })
                    .count();
                if matches == 0 || (exactly_one && matches != 1) {
                    issue(
                        out,
                        node,
                        path,
                        "model_choice",
                        if exactly_one {
                            "Exactly one required-property alternative must be present"
                        } else {
                            "At least one required-property alternative must be present"
                        },
                        Severity::Error,
                    );
                }
            }
        }
    }
    if let Some(items) = node.items() {
        if let Some(item_schema) = s.get("items") {
            for (i, item) in items.iter().enumerate() {
                validate_node(item, item_schema, &format!("{path}[{i}]"), out);
            }
        }
    }
    if let Some(pairs) = node.pairs() {
        if let Some(required) = s.get("required").and_then(|v| v.as_array()) {
            for key in required.iter().filter_map(|v| v.as_str()) {
                if node.get(key).is_none() {
                    issue(
                        out,
                        node,
                        path,
                        "model_required",
                        &format!("Missing required property {key}"),
                        Severity::Error,
                    );
                }
            }
        }
        let mut seen = BTreeSet::new();
        for (k, v) in pairs {
            let key = k.text().unwrap_or("");
            let child_path = format!("{path}.{key}");
            // Duplicate execution fields are intentional. Other repeats are ambiguous, but retained.
            if !seen.insert(key) && path != "$.search.fields" {
                issue(
                    out,
                    k,
                    &child_path,
                    "duplicate_property",
                    "Repeated property retained; typed convenience access returns first value",
                    Severity::Warning,
                );
            }
            let child_schema = s.get("properties").and_then(|p| p.get(key));
            if let Some(child_schema) = child_schema {
                validate_node(v, child_schema, &child_path, out);
            } else if let Some(patterns) = s.get("patternProperties").and_then(|v| v.as_object()) {
                // Dynamic-key blocks share a single value shape, except FieldsBlock whose patterns all reference SelectorBlock.
                if let Some(child_schema) = patterns.values().next() {
                    validate_node(v, child_schema, &child_path, out);
                }
            } else if s.get("additionalProperties") == Some(&serde_json::Value::Bool(false)) {
                issue(
                    out,
                    k,
                    &child_path,
                    "unknown_property",
                    "Property is outside pinned schema; raw node retained",
                    Severity::Warning,
                );
            }
        }
    }
}
/// Model validation, not a general JSON Schema implementation or executable support decision.
pub fn validate(root: &Node) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    validate_node(root, &schema()["definitions"]["SchemaRoot"], "$", &mut out);
    if let Some(id) = root.get("id") {
        if !id.text().is_some_and(|s| {
            !s.is_empty()
                && s.bytes().all(|c| {
                    c.is_ascii_alphabetic() || c.is_ascii_digit() || c == b'-' || c == b'_'
                })
        }) {
            issue(
                &mut out,
                id,
                "$.id",
                "definition_id",
                "ID must contain ASCII letters, digits, hyphen or underscore",
                Severity::Error,
            );
        }
    }
    if let Some(links) = root.get("links").and_then(Node::items) {
        if links.is_empty() {
            issue(
                &mut out,
                root,
                "$.links",
                "links_empty",
                "At least one site link is required",
                Severity::Error,
            );
        }
        for link in links {
            if !link
                .text()
                .and_then(|s| url::Url::parse(s).ok())
                .is_some_and(|u| {
                    matches!(u.scheme(), "http" | "https")
                        && u.host_str().is_some()
                        && u.username().is_empty()
                        && u.password().is_none()
                })
            {
                issue(
                    &mut out,
                    link,
                    "$.links",
                    "link_url",
                    "Expected absolute HTTP(S) site URL without credentials",
                    Severity::Error,
                );
            }
        }
    }
    if let Some(delay) = root.get("requestDelay") {
        if delay
            .text()
            .is_some_and(|s| s.parse::<f64>().is_ok_and(|n| n < 0.0))
        {
            issue(
                &mut out,
                delay,
                "$.requestDelay",
                "request_delay",
                "Request delay must be nonnegative",
                Severity::Error,
            );
        }
    }
    out
}
/// Structural inventory: all required DSL property paths, named filters, field names and templates.
/// This is evidence for the next interpreter phase, never a list of implemented operations.
pub fn features(root: &Node) -> BTreeSet<String> {
    fn walk(n: &Node, path: &str, out: &mut BTreeSet<String>) {
        if let Some(text) = n.text() {
            if text.contains("{{") {
                out.insert("template:go".into());
            }
        }
        if let Some(items) = n.items() {
            for item in items {
                walk(item, &format!("{path}[]"), out);
            }
        }
        if let Some(pairs) = n.pairs() {
            for (k, v) in pairs {
                let key = k.text().unwrap_or("");
                let p = if path.is_empty() {
                    key.to_owned()
                } else {
                    format!("{path}.{key}")
                };
                out.insert(format!("dsl:{p}"));
                if key == "name"
                    && (path.ends_with("filters[]") || path.ends_with("keywordsfilters[]"))
                {
                    if let Some(name) = v.text() {
                        out.insert(format!("filter:{name}"));
                    }
                }
                if path == "search.fields" {
                    out.insert(format!("field:{key}"));
                }
                if key == "encoding" || key == "method" || (key == "type" && !path.is_empty()) {
                    if let Some(value) = v.text() {
                        out.insert(format!("value:{p}={value}"));
                    }
                }
                walk(v, p.trim_start_matches('.'), out);
            }
        }
    }
    let mut out = BTreeSet::from(["execution:unsupported".into()]);
    walk(root, "", &mut out);
    out
}
#[derive(Debug, Serialize)]
pub struct CheckResult {
    pub file: String,
    pub id: Option<String>,
    pub model_valid: bool,
    pub execution_supported: bool,
    pub diagnostics: Vec<Diagnostic>,
    pub required_features: BTreeSet<String>,
}
#[derive(Debug, Serialize)]
pub struct CheckReport {
    pub definitions: Vec<CheckResult>,
    pub summary: Summary,
}
#[derive(Debug, Serialize)]
pub struct Summary {
    pub total: usize,
    pub valid: usize,
    pub invalid: usize,
    pub executable: usize,
    pub required_features: BTreeSet<String>,
}
/// Nonrecursive directory check (sorted .yml/.yaml files), or one explicitly named file.
pub fn check_path(path: &Path, limits: Limits) -> std::io::Result<CheckReport> {
    let mut paths = if path.is_dir() {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let p = entry.path();
            if p.extension().is_some_and(|e| e == "yml" || e == "yaml")
                && entry.file_type()?.is_file()
            {
                files.push(p);
            }
        }
        files
    } else {
        vec![path.to_owned()]
    };
    paths.sort();
    let mut definitions = Vec::new();
    let mut ids = std::collections::BTreeMap::<String, usize>::new();
    for path in paths {
        let (id, mut diagnostics, required_features) = match load(&path, limits) {
            Ok(root) => (
                root.string("id").map(str::to_owned),
                validate(&root),
                features(&root),
            ),
            Err(d) => (None, vec![d], BTreeSet::new()),
        };
        if let Some(id) = &id {
            if let Some(&previous) = ids.get(id) {
                let d = diagnostic(
                    "duplicate_id",
                    "Definition ID is shared by multiple files in this check",
                    None,
                );
                let previous: &mut CheckResult = &mut definitions[previous];
                previous.diagnostics.push(d.clone());
                previous.model_valid = false;
                diagnostics.push(d);
            } else {
                ids.insert(id.clone(), definitions.len());
            }
        }
        let model_valid = !diagnostics.iter().any(|d| d.severity == Severity::Error);
        diagnostics.push(Diagnostic { severity: Severity::Info, code:"execution_unsupported".into(),path:"$".into(),location:None,message:"Model parsing does not enable tracker execution; Cardigann interpreter is not implemented".into() });
        definitions.push(CheckResult {
            file: path.to_string_lossy().into_owned(),
            id,
            model_valid,
            execution_supported: false,
            diagnostics,
            required_features,
        });
    }
    let valid = definitions.iter().filter(|d| d.model_valid).count();
    let required_features = definitions
        .iter()
        .flat_map(|d| d.required_features.iter().cloned())
        .collect();
    Ok(CheckReport {
        summary: Summary {
            total: definitions.len(),
            valid,
            invalid: definitions.len() - valid,
            executable: 0,
            required_features,
        },
        definitions,
    })
}

view!(Request, path => "path", method => "method");
view!(Filter, name => "name");
view!(CategoryMapping, id => "id", category => "cat", description => "desc");
view!(Rows, selector => "selector");
impl<'a> Metadata<'a> {
    pub fn replaces(&self) -> &'a [Node] {
        self.raw
            .get("replaces")
            .and_then(Node::items)
            .unwrap_or_default()
    }
    pub fn links(&self) -> &'a [Node] {
        self.raw
            .get("links")
            .and_then(Node::items)
            .unwrap_or_default()
    }
    pub fn legacy_links(&self) -> &'a [Node] {
        self.raw
            .get("legacylinks")
            .and_then(Node::items)
            .unwrap_or_default()
    }
}
impl<'a> Setting<'a> {
    pub fn options(&self) -> &'a [(Node, Node)] {
        self.raw
            .get("options")
            .and_then(Node::pairs)
            .unwrap_or_default()
    }
    pub fn defaults(&self) -> &'a [Node] {
        self.raw
            .get("defaults")
            .and_then(Node::items)
            .unwrap_or_default()
    }
}
impl<'a> Selector<'a> {
    pub fn optional(&self) -> bool {
        self.raw.string("optional") == Some("true")
    }
    pub fn filters(&self) -> impl Iterator<Item = Filter<'a>> {
        self.raw
            .get("filters")
            .and_then(Node::items)
            .unwrap_or_default()
            .iter()
            .map(|raw| Filter { raw })
    }
    pub fn cases(&self) -> &'a [(Node, Node)] {
        self.raw
            .get("case")
            .and_then(Node::pairs)
            .unwrap_or_default()
    }
}
impl<'a> Request<'a> {
    pub fn inherit_inputs(&self) -> bool {
        self.raw.string("inheritinputs") != Some("false")
    }
    pub fn follow_redirect(&self) -> bool {
        self.raw.string("followredirect") == Some("true")
    }
    pub fn query_separator(&self) -> &'a str {
        self.raw.string("queryseparator").unwrap_or("&")
    }
    pub fn inputs(&self) -> &'a [(Node, Node)] {
        self.raw
            .get("inputs")
            .and_then(Node::pairs)
            .unwrap_or_default()
    }
    pub fn response_type(&self) -> &'a str {
        self.raw
            .get("response")
            .and_then(|n| n.string("type"))
            .unwrap_or("html")
    }
}
impl<'a> Search<'a> {
    pub fn requests(&self) -> impl Iterator<Item = Request<'a>> {
        let legacy = self.raw.get("path").map(|_| Request { raw: self.raw });
        legacy
            .into_iter()
            .chain(self.paths().iter().map(|raw| Request { raw }))
    }
    pub fn row_definition(&self) -> Option<Rows<'a>> {
        self.raw.get("rows").map(|raw| Rows { raw })
    }
    pub fn inputs(&self) -> &'a [(Node, Node)] {
        self.raw
            .get("inputs")
            .and_then(Node::pairs)
            .unwrap_or_default()
    }
}
impl<'a> Login<'a> {
    pub fn inputs(&self) -> &'a [(Node, Node)] {
        self.raw
            .get("inputs")
            .and_then(Node::pairs)
            .unwrap_or_default()
    }
    pub fn selectors(&self) -> bool {
        self.raw.string("selectors") == Some("true")
    }
}
impl<'a> Download<'a> {
    pub fn before(&self) -> Option<Request<'a>> {
        self.raw.get("before").map(|raw| Request { raw })
    }
    pub fn selectors(&self) -> impl Iterator<Item = Selector<'a>> {
        self.raw
            .get("selectors")
            .and_then(Node::items)
            .unwrap_or_default()
            .iter()
            .map(|raw| Selector { raw })
    }
    pub fn infohash(&self) -> Option<&'a Node> {
        self.raw.get("infohash")
    }
}
impl Rows<'_> {
    pub fn after(&self) -> usize {
        self.raw
            .string("after")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }
    pub fn multiple(&self) -> bool {
        self.raw.string("multiple") == Some("true")
    }
    pub fn missing_attribute_equals_no_results(&self) -> bool {
        self.raw.string("missingAttributeEqualsNoResults") == Some("true")
    }
}
