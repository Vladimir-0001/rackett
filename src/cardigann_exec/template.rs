//! Bounded, owned Cardigann template programs. Expressions follow Cardigann's
//! Go-looking template helpers, rather than evaluating arbitrary Go code.
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::BTreeMap;

pub const MAX_OUTPUT: usize = 1024 * 1024;
#[derive(Clone, Debug)]
pub struct Context {
    pub values: BTreeMap<String, Value>,
    pub now: DateTime<Utc>,
}
#[derive(Clone, Debug)]
pub struct Template {
    nodes: Vec<Node>,
}
#[derive(Clone, Debug)]
enum Node {
    Text(String),
    Value(Expr),
    If(Expr, Vec<Node>, Vec<Node>),
    Range(Expr, Vec<Node>, Option<String>),
}
#[derive(Clone, Debug)]
enum Expr {
    Literal(String),
    Variable(String),
    Call(String, Vec<Expr>),
    RegexReplace(Box<Expr>, super::filters::DefinitionRegex, Box<Expr>),
}
fn tokens(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        if c == '(' || c == ')' {
            out.push(c.to_string());
            continue;
        }
        let mut t = String::from(c);
        if c == '"' {
            let mut escaped = false;
            let mut closed = false;
            for c in chars.by_ref() {
                t.push(c);
                if c == '"' && !escaped {
                    closed = true;
                    break;
                }
                escaped = c == '\\' && !escaped;
            }
            if !closed {
                return Err("unterminated template string".into());
            }
        } else {
            while let Some(c) = chars.peek() {
                if c.is_whitespace() || *c == '(' || *c == ')' {
                    break;
                }
                t.push(chars.next().unwrap());
            }
        }
        out.push(t);
    }
    Ok(out)
}
fn expression(ts: &[String], pos: &mut usize, depth: usize) -> Result<Expr, String> {
    if depth > 24 {
        return Err("template expression depth exceeded".into());
    }
    let t = ts.get(*pos).ok_or("missing expression")?.clone();
    *pos += 1;
    if t == "(" {
        let e = expression(ts, pos, depth + 1)?;
        if ts.get(*pos).map(String::as_str) != Some(")") {
            return Err("unclosed expression".into());
        }
        *pos += 1;
        return Ok(e);
    }
    if t.starts_with('"') {
        let value = t
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .ok_or("unterminated template literal")?;
        if value.contains('"') {
            return Err(
                "escaped quotes in template literals are not supported by Cardigann".into(),
            );
        }
        // Cardigann's C# regex captures literal interiors verbatim. In particular,
        // regex \b must remain a backslash and b, not a JSON backspace escape.
        return Ok(Expr::Literal(value.to_owned()));
    }
    if t.starts_with('.') || t.starts_with('$') {
        return Ok(Expr::Variable(t));
    }
    let arity = match t.as_str() {
        "eq" | "ne" | "join" => 2,
        "re_replace" => 3,
        "and" | "or" => 2,
        _ => return Err(format!("unsupported template expression {t}")),
    };
    let mut args = Vec::new();
    for _ in 0..arity {
        args.push(expression(ts, pos, depth + 1)?);
    }
    if t == "and" || t == "or" {
        while *pos < ts.len() && ts[*pos] != ")" {
            args.push(expression(ts, pos, depth + 1)?);
        }
    }
    if t == "re_replace" {
        let regex = if let Expr::Literal(pattern) = &args[1] {
            super::filters::definition_regex(pattern)?
        } else {
            return Err("regex pattern must be literal".into());
        };
        let mut iter = args.into_iter();
        let input = iter.next().unwrap();
        iter.next();
        let replacement = iter.next().unwrap();
        return Ok(Expr::RegexReplace(
            Box::new(input),
            regex,
            Box::new(replacement),
        ));
    }
    Ok(Expr::Call(t, args))
}
fn parse_expr(s: &str) -> Result<Expr, String> {
    let ts = tokens(s)?;
    let mut p = 0;
    let e = expression(&ts, &mut p, 0)?;
    if p != ts.len() {
        return Err("trailing template expression".into());
    }
    Ok(e)
}
fn parse_nodes(parts: &[(bool, String)], p: &mut usize, depth: usize) -> Result<Vec<Node>, String> {
    if depth > 24 {
        return Err("template block depth exceeded".into());
    }
    let mut out = Vec::new();
    while let Some((tag, s)) = parts.get(*p) {
        if *tag && (s == "else" || s == "end") {
            break;
        }
        *p += 1;
        if !tag {
            out.push(Node::Text(s.clone()));
        } else if let Some(condition) = s.strip_prefix("if ") {
            let e = parse_expr(condition)?;
            let yes = parse_nodes(parts, p, depth + 1)?;
            let no = if parts.get(*p).map(|x| x.1.as_str()) == Some("else") {
                *p += 1;
                parse_nodes(parts, p, depth + 1)?
            } else {
                Vec::new()
            };
            if parts.get(*p).map(|x| x.1.as_str()) != Some("end") {
                return Err("unclosed if".into());
            }
            *p += 1;
            out.push(Node::If(e, yes, no));
        } else if let Some(spec) = s.strip_prefix("range ") {
            let (index, expr) = if let Some((bindings, expr)) = spec.split_once(":=") {
                let (index, _element) = bindings
                    .split_once(',')
                    .ok_or("range requires index, element bindings")?;
                (Some(index.trim().to_string()), expr.trim())
            } else {
                (None, spec)
            };
            let e = parse_expr(expr)?;
            let body = parse_nodes(parts, p, depth + 1)?;
            if parts.get(*p).map(|x| x.1.as_str()) != Some("end") {
                return Err("unclosed range".into());
            }
            *p += 1;
            out.push(Node::Range(e, body, index));
        } else {
            out.push(Node::Value(parse_expr(s)?));
        }
    }
    Ok(out)
}
/// Stand-ins for a configured `:contains` value while proving static variants:
/// one identifier and one quoted string, both inside [`super::css::contains_value`].
const SENTINELS: [&str; 2] = ["rackettconfigvalue", "\"rackett config value\""];
const VALUE_TYPE: &str = "row selector configuration value must be text";
const VALUE_POSITION: &str =
    "row selector configuration value must be the whole argument of :contains(...)";
const VALUE_SELECTOR: &str = "row selector with configuration value is not a supported selector";
const VALUE_JSON: &str =
    "JSON row selector configuration value must not contain parentheses or controls";
/// One enumerated row selector branch: literal text and value slots.
#[derive(Clone)]
enum Piece {
    Text(String),
    Value,
}
fn config_key(key: &str) -> bool {
    key.strip_prefix(".Config.").is_some_and(|name| {
        !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    })
}
/// Render `pieces` with `value` in each slot, returning the slot byte ranges.
fn fill(pieces: &[Piece], value: &str) -> (String, Vec<std::ops::Range<usize>>) {
    let mut out = String::new();
    let mut ranges = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Text(text) => out.push_str(text),
            Piece::Value => {
                let start = out.len();
                out.push_str(value);
                ranges.push(start..out.len());
            }
        }
    }
    (out, ranges)
}
impl Template {
    /// Enumerate a bounded row selector language before admitting a plan.
    /// Both arms must be supported, regardless of current setting defaults.
    ///
    /// Pinned Jackett resolves flat single-line `{{ if .Var }}a{{ else }}b{{ end }}`
    /// blocks first (`CardigannIndexer.cs:438-478`), then substitutes simple
    /// variables raw (`CardigannIndexer.cs:517-530`). The only substitution
    /// admitted here is the guard's own `.Config.name` inside that guard's true
    /// arm, where it must be the whole argument of a `:contains(...)`; every
    /// variant is proven with identifier and string stand-ins. The returned
    /// variants hold each stand-in, for the caller's full selector validation.
    pub(super) fn compile_selector(source: &str) -> Result<(Self, Vec<String>), String> {
        // Inspect raw conditions before the shared parser removes parentheses.
        // C# requires the captured condition to start with '.'.
        for tag in source.split("{{").skip(1) {
            let tag = tag.split("}}").next().unwrap_or_default().trim();
            if let Some(condition) = tag.strip_prefix("if ") {
                if !condition.trim_start().starts_with(".Config.") {
                    return Err(
                        "row selector conditions must be bare configuration variables".into(),
                    );
                }
            }
        }
        fn variants(nodes: &[Node], guard: Option<&str>) -> Result<Vec<Vec<Piece>>, String> {
            let mut out = vec![(Vec::new(), 0usize)];
            for node in nodes {
                let suffixes = match node {
                    Node::Text(text) => vec![vec![Piece::Text(text.clone())]],
                    Node::Value(Expr::Variable(key)) if guard == Some(key.as_str()) => {
                        vec![vec![Piece::Value]]
                    }
                    Node::Value(Expr::Variable(_)) => {
                        return Err("row selector configuration values must be guarded by their own if block".into())
                    }
                    Node::If(Expr::Variable(key), yes, no)
                        if key.starts_with(".Config.") =>
                    {
                        // The pinned C# if/else helper is a non-Singleline regex,
                        // not a recursive Go parser. Keep this subset faithful.
                        let flat = |node: &Node| {
                            matches!(node, Node::Text(text) if !text.contains(['\r', '\n']))
                        };
                        if no.is_empty()
                            || !yes.iter().chain(no).all(|node| {
                                flat(node) || matches!(node, Node::Value(Expr::Variable(_)))
                            })
                        {
                            return Err("row selectors require flat single-line if/else blocks".into());
                        }
                        let slots = yes.iter().any(|node| !flat(node));
                        if slots && !config_key(key) {
                            return Err("row selector configuration value names must be plain".into());
                        }
                        let mut arms = variants(yes, Some(key))?;
                        arms.extend(variants(no, None)?);
                        arms
                    }
                    _ => return Err("row selectors support only literal text and configuration-variable if/else blocks".into()),
                };
                if out.len().saturating_mul(suffixes.len()) > 64 {
                    return Err("row selector variant count limit exceeded".into());
                }
                let mut next = Vec::new();
                for (prefix, length) in &out {
                    for suffix in &suffixes {
                        let added = suffix.iter().map(|piece| match piece {
                            Piece::Text(text) => text.len(),
                            Piece::Value => SENTINELS.iter().map(|s| s.len()).max().unwrap_or(0),
                        });
                        let length = added.fold(*length, usize::saturating_add);
                        if length > 4096 {
                            return Err("row selector length limit exceeded".into());
                        }
                        let mut pieces = prefix.clone();
                        pieces.extend(suffix.iter().cloned());
                        next.push((pieces, length));
                    }
                }
                out = next;
            }
            Ok(out.into_iter().map(|(pieces, _)| pieces).collect())
        }
        let template = Self::compile(source)?;
        let mut rendered = Vec::new();
        for pieces in variants(&template.nodes, None)? {
            for sentinel in SENTINELS {
                let (variant, ranges) = fill(&pieces, sentinel);
                let literal = ranges.is_empty();
                if !literal {
                    super::selector::check_css_bounds(&variant)?;
                    if !super::css::contains_arguments(&variant, &ranges) {
                        return Err(VALUE_POSITION.into());
                    }
                }
                rendered.push(variant);
                if literal {
                    break;
                }
            }
        }
        Ok((template, rendered))
    }
    /// `.Config` names substituted into this row selector.
    pub(super) fn selector_values(&self) -> Vec<&str> {
        let mut keys = Vec::new();
        for node in &self.nodes {
            if let Node::If(Expr::Variable(key), yes, _) = node {
                if yes.iter().any(|node| matches!(node, Node::Value(_)))
                    && !keys.contains(&key.as_str())
                {
                    keys.push(key.as_str());
                }
            }
        }
        keys
    }
    /// Render a [`Template::compile_selector`] program for these settings.
    /// Without substitutions this is [`Template::render`]. Otherwise each
    /// value must be text, one CSS identifier or string, and again the whole
    /// `:contains` argument in the actual rendered selector, which must stay
    /// within the CSS bounds and parse. Errors never repeat the value.
    pub(super) fn render_selector(&self, ctx: &Context) -> Result<String, String> {
        self.render_row_selector(ctx, false)
    }
    /// [`Template::render_selector`] for a JSON rows selector. Upstream renders it
    /// with the configuration variables and splits it into a token path and
    /// `:has`/`:not`/`:contains` filters (`JsonParseRowsSelector`,
    /// `CardigannIndexer.cs:2491-2562`, regex at `:73`). A value is literal
    /// `:contains` text there, so it is not CSS-checked; parentheses, which would
    /// let the upstream regex and Rackett's parser split it differently, and
    /// controls are rejected. The result must parse as a JSON selector.
    pub(super) fn render_json_selector(&self, ctx: &Context) -> Result<String, String> {
        self.render_row_selector(ctx, true)
    }
    fn render_row_selector(&self, ctx: &Context, json: bool) -> Result<String, String> {
        if self.selector_values().is_empty() {
            return self.render(ctx);
        }
        validate_context(ctx)?;
        let mut out = String::new();
        let mut ranges = Vec::new();
        for node in &self.nodes {
            let (key, yes, no) = match node {
                Node::Text(text) => {
                    out.push_str(text);
                    continue;
                }
                Node::If(Expr::Variable(key), yes, no) => (key, yes, no),
                _ => return Err("unsupported row selector program".into()),
            };
            let value = ctx
                .values
                .get(key)
                .ok_or_else(|| format!("missing template variable {key}"))?;
            let substitutes = yes.iter().any(|node| matches!(node, Node::Value(_)));
            // C# IsNullOrWhiteSpace selects the arm; any other type throws or
            // is cast upstream, so it is rejected rather than coerced.
            let arm = match value {
                Value::Null if substitutes => no,
                Value::String(text) if substitutes => {
                    if text.trim().is_empty() {
                        no
                    } else {
                        if json {
                            if text
                                .chars()
                                .any(|c| c.is_control() || matches!(c, '(' | ')'))
                            {
                                return Err(VALUE_JSON.into());
                            }
                        } else {
                            super::css::contains_value(text)?;
                        }
                        yes
                    }
                }
                _ if substitutes => return Err(VALUE_TYPE.into()),
                _ if truth(value) => yes,
                _ => no,
            };
            for node in arm {
                match (node, value) {
                    (Node::Text(text), _) => out.push_str(text),
                    (Node::Value(Expr::Variable(name)), Value::String(text)) if name == key => {
                        let start = out.len();
                        out.push_str(text);
                        ranges.push(start..out.len());
                    }
                    _ => return Err("unsupported row selector program".into()),
                }
                if out.len() > MAX_OUTPUT {
                    return Err("template output limit exceeded".into());
                }
            }
        }
        if ranges.is_empty() {
            return Ok(out);
        }
        super::selector::check_css_bounds(&out)?;
        if json {
            super::selector::json_select(&Value::Null, &out).map_err(|_| VALUE_SELECTOR)?;
            return Ok(out);
        }
        if !super::css::contains_arguments(&out, &ranges) {
            return Err(VALUE_POSITION.into());
        }
        super::css::Css::parse(&out).map_err(|_| VALUE_SELECTOR)?;
        Ok(out)
    }
    pub fn compile(s: &str) -> Result<Self, String> {
        if s.len() > 65536 {
            return Err("template source limit exceeded".into());
        }
        let mut parts = Vec::new();
        let mut rest = s;
        while let Some(start) = rest.find("{{") {
            parts.push((false, rest[..start].into()));
            rest = &rest[start + 2..];
            let end = rest.find("}}").ok_or("unclosed template tag")?;
            parts.push((true, rest[..end].trim().into()));
            rest = &rest[end + 2..];
        }
        parts.push((false, rest.into()));
        if parts.len() > 2048 {
            return Err("template node limit exceeded".into());
        }
        let mut p = 0;
        let nodes = parse_nodes(&parts, &mut p, 0)?;
        if p != parts.len() {
            return Err("unexpected template end/else".into());
        }
        Ok(Self { nodes })
    }
    pub fn render(&self, ctx: &Context) -> Result<String, String> {
        validate_context(ctx)?;
        let mut output = String::new();
        let mut budget = 16384;
        render_nodes(&self.nodes, ctx, &mut output, &mut budget, 0)?;
        Ok(output)
    }
    /// Encode substituted values only, preserving literal raw query grammar.
    pub fn render_raw(&self, ctx: &Context) -> Result<String, String> {
        validate_context(ctx)?;
        let mut output = String::new();
        let mut budget = 16384;
        render_nodes(&self.nodes, ctx, &mut output, &mut budget, 2)?;
        Ok(output)
    }
    pub fn render_path(&self, ctx: &Context) -> Result<String, String> {
        validate_context(ctx)?;
        let mut output = String::new();
        let mut budget = 16384;
        render_nodes(&self.nodes, ctx, &mut output, &mut budget, 1)?;
        Ok(output)
    }
}
fn validate_context(ctx: &Context) -> Result<(), String> {
    if ctx.values.len() > 65536 {
        return Err("template context entry limit exceeded".into());
    }
    let mut stack = ctx.values.values().map(|v| (v, 0usize)).collect::<Vec<_>>();
    let mut count = 0usize;
    let mut bytes = 0usize;
    while let Some((value, depth)) = stack.pop() {
        count += 1;
        if count > 65536 || depth > 64 {
            return Err("template context structure limit exceeded".into());
        }
        match value {
            Value::String(s) => bytes = bytes.saturating_add(s.len()),
            Value::Array(a) => {
                if a.len().saturating_add(stack.len()).saturating_add(count) > 65536 {
                    return Err("template context item limit exceeded".into());
                }
                stack.extend(a.iter().map(|v| (v, depth + 1)));
            }
            Value::Object(o) => {
                if o.len().saturating_add(stack.len()).saturating_add(count) > 65536 {
                    return Err("template context item limit exceeded".into());
                }
                bytes = bytes.saturating_add(o.keys().map(String::len).sum::<usize>());
                stack.extend(o.values().map(|v| (v, depth + 1)));
            }
            _ => {}
        }
        if bytes > 4 * MAX_OUTPUT {
            return Err("template context byte limit exceeded".into());
        }
    }
    Ok(())
}

fn string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        _ => v.to_string(),
    }
}
fn truth(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::String(s) => !s.trim().is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Bool(v) => *v,
        _ => true,
    }
}
fn eval(e: &Expr, ctx: &Context) -> Result<Value, String> {
    match e {
        Expr::Literal(s) => Ok(Value::String(s.clone())),
        Expr::Variable(k) => {
            if k == ".True" {
                return Ok(Value::String("True".into()));
            }
            if k == ".False" {
                return Ok(Value::Null);
            }
            ctx.values
                .get(k)
                .cloned()
                .ok_or_else(|| format!("missing template variable {k}"))
        }
        Expr::RegexReplace(input, re, replacement) => {
            Ok(Value::String(super::filters::replace_regex(
                re,
                &string(&eval(input, ctx)?),
                &string(&eval(replacement, ctx)?),
            )?))
        }
        Expr::Call(name, args) => {
            let v = args
                .iter()
                .map(|e| eval(e, ctx))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(match name.as_str() {
                "eq" => Value::Bool(v[0] == v[1]),
                "ne" => Value::Bool(v[0] != v[1]),
                "and" => v
                    .iter()
                    .find(|x| !truth(x))
                    .unwrap_or(v.last().unwrap())
                    .clone(),
                "or" => v
                    .iter()
                    .find(|x| truth(x))
                    .unwrap_or(v.last().unwrap())
                    .clone(),
                "join" => {
                    let a = v[0].as_array().ok_or("join expects array")?;
                    let len = a
                        .iter()
                        .map(|v| string(v).len())
                        .sum::<usize>()
                        .saturating_add(a.len().saturating_mul(string(&v[1]).len()));
                    if len > MAX_OUTPUT {
                        return Err("join output limit exceeded".into());
                    }
                    Value::String(
                        a.iter()
                            .map(string)
                            .collect::<Vec<_>>()
                            .join(&string(&v[1])),
                    )
                }
                _ => unreachable!(),
            })
        }
    }
}
fn render_nodes(
    nodes: &[Node],
    ctx: &Context,
    out: &mut String,
    budget: &mut usize,
    encode: u8,
) -> Result<(), String> {
    for n in nodes {
        if *budget == 0 {
            return Err("template operation limit exceeded".into());
        }
        *budget -= 1;
        match n {
            Node::Text(s) => out.push_str(s),
            Node::Value(e) => {
                let value = string(&eval(e, ctx)?);
                if encode != 0 {
                    let encoded = super::filters::url_encode(&value);
                    out.push_str(&if encode == 1 {
                        encoded.replace('+', "%20")
                    } else {
                        encoded
                    });
                } else {
                    out.push_str(&value);
                }
            }
            Node::If(e, y, n) => render_nodes(
                if truth(&eval(e, ctx)?) { y } else { n },
                ctx,
                out,
                budget,
                encode,
            )?,
            Node::Range(e, body, index) => {
                let value = eval(e, ctx)?;
                let array = value.as_array().ok_or("range expects array")?;
                if array.len() > 4096 {
                    return Err("range item limit exceeded".into());
                }
                let mut local = ctx.clone();
                for (i, v) in array.iter().enumerate() {
                    local.values.insert(".".into(), v.clone());
                    if let Some(k) = index {
                        local.values.insert(k.clone(), Value::String(i.to_string()));
                    }
                    render_nodes(body, &local, out, budget, encode)?;
                }
            }
        }
        if out.len() > MAX_OUTPUT {
            return Err("template output limit exceeded".into());
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn branches_and_range() {
        let ctx = Context {
            values: BTreeMap::from([
                (".Query.Q".into(), Value::String("hello".into())),
                (".Categories".into(), serde_json::json!(["1", "2"])),
            ]),
            now: Utc::now(),
        };
        assert_eq!(Template::compile("{{if eq .Query.Q \"hello\"}}{{range $i, $e := .Categories}}{{$i}}={{.}};{{end}}{{else}}no{{end}}").unwrap().render(&ctx).unwrap(),"0=1;1=2;");
        assert!(Template::compile("{{unknown .Q}}").is_err());
    }
    #[test]
    fn literals_preserve_regex_backslashes() {
        let ctx = Context {
            values: BTreeMap::from([(".Query.Q".into(), Value::String("foo food".into()))]),
            now: Utc::now(),
        };
        assert_eq!(
            Template::compile(r#"{{ re_replace .Query.Q "\bfoo\b" "X" }}"#)
                .unwrap()
                .render(&ctx)
                .unwrap(),
            "X food"
        );
        assert!(Template::compile(r#"{{eq .Query.Q "a\"b"}}"#).is_err());
        assert_eq!(
            Template::compile(r#"{{join .Categories "\n"}}"#)
                .unwrap()
                .render(&Context {
                    values: BTreeMap::from([(".Categories".into(), serde_json::json!(["a", "b"]))]),
                    now: ctx.now
                })
                .unwrap(),
            r"a\nb"
        );
    }
    #[test]
    fn untrusted_context_and_output_limits() {
        let mut nested = Value::String("x".into());
        for _ in 0..65 {
            nested = Value::Array(vec![nested]);
        }
        let mut ctx = Context {
            values: BTreeMap::from([(".Q".into(), nested)]),
            now: Utc::now(),
        };
        assert!(Template::compile("{{.Q}}").unwrap().render(&ctx).is_err());
        ctx.values.insert(
            ".Q".into(),
            Value::Array(vec![Value::String("x".into()); 4097]),
        );
        assert!(Template::compile("{{range .Q}}{{.}}{{end}}")
            .unwrap()
            .render(&ctx)
            .is_err());
        ctx.values
            .insert(".Q".into(), Value::String("x".repeat(MAX_OUTPUT)));
        assert!(Template::compile("{{.Q}}{{.Q}}")
            .unwrap()
            .render(&ctx)
            .is_err());
    }
}
