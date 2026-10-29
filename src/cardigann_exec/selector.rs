//! Bounded Cardigann HTML and JSON selectors with response-owned DOM mutation.
use super::css::{Css, Scan};
use crate::cardigann::Node;
use scraper::{ElementRef, Html};
use serde_json::Value;
const MAX_SELECTOR: usize = 4096;
const MAX_VISITS: usize = 100_000;
const MAX_OUTPUT: usize = 1_048_576;
#[derive(Clone, Debug)]
pub struct Selector {
    selector: Option<String>,
    text: Option<String>,
    attribute: Option<String>,
    remove: Option<String>,
    cases: Vec<(String, String)>,
}
impl Selector {
    pub fn compile(node: &Node) -> Result<Self, String> {
        let mut s = Self {
            selector: None,
            text: None,
            attribute: None,
            remove: None,
            cases: vec![],
        };
        for (k, v) in node.pairs().ok_or("selector must be mapping")? {
            let key = k.text().ok_or("selector key must be scalar")?;
            let target = match key {
                "selector" => &mut s.selector,
                "text" => &mut s.text,
                "attribute" => &mut s.attribute,
                "remove" => &mut s.remove,
                "case" => {
                    for (k, v) in v.pairs().ok_or("case must be mapping")? {
                        s.cases.push((
                            k.text().ok_or("case key must be scalar")?.into(),
                            v.text().ok_or("case value must be scalar")?.into(),
                        ));
                    }
                    continue;
                }
                "filters" | "optional" | "default" => continue,
                _ => return Err(format!("unsupported selector key {key}")),
            };
            if target.is_some() {
                return Err(format!("duplicate selector property {key}"));
            }
            *target = Some(v.text().ok_or("selector property must be scalar")?.into());
        }
        Ok(s)
    }
    pub fn validate_html(&self) -> Result<(), String> {
        for s in self
            .selector
            .iter()
            .chain(self.cases.iter().map(|(k, _)| k))
        {
            if !s.contains("{{") {
                let s = s.strip_prefix(":root").map(str::trim).unwrap_or(s);
                if !s.is_empty() {
                    parse_css(s)?;
                } else if self.selector.as_deref() != Some(":root") {
                    return Err("empty CSS selector".into());
                }
            }
        }
        if let Some(remove) = &self.remove {
            parse_css(remove)?;
        }
        Ok(())
    }
    pub fn validate_json(&self) -> Result<(), String> {
        if self.remove.is_some() || self.attribute.is_some() {
            return Err("JSON field attribute/remove unsupported by upstream helper".into());
        }
        if let Some(s) = &self.selector {
            if !s.contains("{{") {
                JsonSelector::parse(s, 0)?;
            }
        }
        Ok(())
    }
    pub fn extract_html<F>(
        &self,
        element: ElementRef<'_>,
        render: &F,
    ) -> Result<Option<String>, String>
    where
        F: Fn(&str) -> Result<String, String>,
    {
        if let Some(s) = &self.text {
            return Ok(Some(bounded(render(s)?)?));
        }
        if self.remove.is_some() {
            let row = element
                .tree()
                .nodes()
                .position(|node| node.id() == element.id())
                .ok_or("HTML row missing")?;
            let mut document = Html {
                tree: element.tree().clone(),
                ..Html::new_document()
            };
            return self.extract_html_mut(&mut document, row, &mut 20_000_000, render);
        }
        let selected = match &self.selector {
            Some(s) => html_select(element, &render(s)?, true)?.into_iter().next(),
            None => Some(element),
        };
        let Some(selected) = selected else {
            return Ok(None);
        };
        if !self.cases.is_empty() {
            for (k, v) in &self.cases {
                if !html_select(selected, k, true)?.is_empty() {
                    return Ok(Some(bounded(normalize(&render(v)?))?));
                }
            }
            return Ok(None);
        }
        if let Some(a) = &self.attribute {
            // AngleSharp Element.GetAttribute lowercases names for HTML elements
            // only. Foreign (SVG/MathML) attribute names remain case-sensitive.
            let name = if selected.value().name.ns.as_ref() == "http://www.w3.org/1999/xhtml" {
                a.to_ascii_lowercase()
            } else {
                a.clone()
            };
            return selected
                .value()
                .attr(&name)
                .map(|s| bounded(normalize(s)))
                .transpose();
        }
        let mut text = String::new();
        for (count, node) in selected.descendants().enumerate() {
            if count >= MAX_VISITS {
                return Err("HTML selector visit limit exceeded".into());
            }
            if let Some(t) = node.value().as_text() {
                text.push_str(t);
                if text.len() > MAX_OUTPUT {
                    return Err("selector output limit exceeded".into());
                }
            }
        }
        Ok(Some(normalize(&text)))
    }
    /// Mutate the response tree before reading cases/attributes/text. Node positions
    /// remain stable because ego-tree detaches nodes without deleting their slots.
    pub fn extract_html_mut<F>(
        &self,
        document: &mut Html,
        row: usize,
        visits: &mut usize,
        render: &F,
    ) -> Result<Option<String>, String>
    where
        F: Fn(&str) -> Result<String, String>,
    {
        if let Some(text) = &self.text {
            return Ok(Some(bounded(render(text)?)?));
        }
        charge(visits, row.saturating_add(1))?;
        let row_id = document
            .tree
            .nodes()
            .nth(row)
            .ok_or("HTML row missing")?
            .id();
        let element = ElementRef::wrap(document.tree.get(row_id).ok_or("HTML row missing")?)
            .ok_or("HTML row is not an element")?;
        let rendered_selector = self.selector.as_ref().map(|s| render(s)).transpose()?;
        let selection_scope = if rendered_selector
            .as_deref()
            .is_some_and(|s| s.starts_with(":root"))
        {
            let mut root = element;
            while let Some(parent) = root.parent().and_then(ElementRef::wrap) {
                root = parent;
            }
            root
        } else {
            element
        };
        charge(visits, selection_scope.descendants().count())?;
        let selected = match &rendered_selector {
            Some(selector) => html_select(element, selector, true)?.into_iter().next(),
            None => Some(element),
        };
        let Some(selected) = selected else {
            return Ok(None);
        };
        let selected_id = selected.id();
        if let Some(remove) = &self.remove {
            let css = parse_css(remove)?;
            charge(visits, selected.descendants().count())?;
            let scan = Scan::default();
            let mut removals = vec![];
            for node in selected.descendants().filter_map(ElementRef::wrap) {
                if node.id() != selected_id && css.matches(&node, &scan)? {
                    removals.push(node.id());
                }
            }
            for id in removals {
                document
                    .tree
                    .get_mut(id)
                    .ok_or("HTML removal node missing")?
                    .detach();
            }
        }
        let selected = ElementRef::wrap(
            document
                .tree
                .get(selected_id)
                .ok_or("HTML selection missing")?,
        )
        .ok_or("HTML selection missing")?;
        // Selection already happened: do not re-run it after detachment changes siblings.
        let mut reader = self.clone();
        reader.selector = None;
        reader.remove = None;
        let mut scope = selected;
        if self.cases.iter().any(|(s, _)| s.starts_with(":root")) {
            while let Some(parent) = scope.parent().and_then(ElementRef::wrap) {
                scope = parent;
            }
        }
        charge(
            visits,
            scope
                .descendants()
                .count()
                .saturating_mul(self.cases.len().saturating_add(1)),
        )?;
        reader.extract_html(selected, render)
    }
    pub fn extract_json<F>(&self, value: &Value, render: &F) -> Result<Option<String>, String>
    where
        F: Fn(&str) -> Result<String, String>,
    {
        if self.remove.is_some() || self.attribute.is_some() {
            return Err("JSON field attribute/remove unsupported".into());
        }
        if let Some(s) = &self.text {
            return Ok(Some(bounded(render(s)?)?));
        }
        let mut selected = if let Some(s) = &self.selector {
            let Some(found) = json_select(value, &render(s)?)? else {
                return Ok(None);
            };
            Some(json_string(found)?)
        } else {
            None
        };
        for (k, v) in &self.cases {
            if k == "*" || selected.as_deref() == Some(k) {
                selected = Some(render(v)?);
                break;
            }
        }
        selected.map(|s| bounded(normalize(&s))).transpose()
    }
}
fn charge(visits: &mut usize, count: usize) -> Result<(), String> {
    *visits = visits
        .checked_sub(count)
        .ok_or("HTML aggregate visit budget exceeded")?;
    Ok(())
}
/// Snapshot all row slots before mutation, including nested rows later detached.
pub fn html_row_slots(document: &Html, selector: &str, max: usize) -> Result<Vec<usize>, String> {
    let rows = html_rows(document, selector, max)?;
    let slots = document
        .tree
        .nodes()
        .enumerate()
        .map(|(i, n)| (n.id(), i))
        .collect::<std::collections::HashMap<_, _>>();
    Ok(rows.iter().map(|row| slots[&row.id()]).collect())
}
/// Upstream `Rows.After` (CardigannIndexer.cs:1834-1852): each head row in snapshot order
/// receives every child node of the next `after` selected rows, which leave the row list
/// but stay in the DOM, empty. An incomplete final group makes upstream's `RemoveRange`
/// throw, so it fails the whole response. Moving an ancestor of the head throws
/// `HierarchyRequest` upstream (NodeExtensions.cs:408-413) and fails here too. Slots stay
/// valid because arena nodes are moved, never deleted.
pub fn merge_after_rows(
    document: &mut Html,
    slots: Vec<usize>,
    after: usize,
    visits: &mut usize,
) -> Result<Vec<usize>, String> {
    if after == 0 || slots.is_empty() {
        return Ok(slots);
    }
    // A group larger than usize::MAX can never be complete.
    let group = after.checked_add(1).ok_or("Incomplete row merge group")?;
    if !slots.len().is_multiple_of(group) {
        return Err("Incomplete row merge group".into());
    }
    let ids = document.tree.nodes().map(|n| n.id()).collect::<Vec<_>>();
    charge(visits, ids.len())?;
    let id = |slot: usize| ids.get(slot).copied().ok_or("HTML row missing");
    // QuerySelectorAll never reaches template contents upstream, but html_select
    // descends into scraper's Fragment, so such rows would shift every group. Check
    // the original ancestry of every selected row before any move can conceal it.
    for &slot in &slots {
        let row = document.tree.get(id(slot)?).ok_or("HTML row missing")?;
        for ancestor in row.ancestors() {
            charge(visits, 1)?;
            if ancestor.value().is_fragment() {
                return Err("Inert template row merge unsupported".into());
            }
        }
    }
    let mut heads = Vec::with_capacity(slots.len() / group);
    for chunk in slots.chunks(group) {
        let head = id(chunk[0])?;
        let target = document.tree.get(head).ok_or("HTML row missing")?;
        // Template contents are outside ChildNodes upstream and cannot be mirrored here.
        if target
            .first_child()
            .is_some_and(|c| c.value().is_fragment())
        {
            return Err("Template row merge unsupported".into());
        }
        for &slot in &chunk[1..] {
            let source = id(slot)?;
            if source == head {
                return Err("Row merge source is its own target".into());
            }
            let target = document.tree.get(head).ok_or("HTML row missing")?;
            for ancestor in target.ancestors() {
                charge(visits, 1)?;
                if ancestor.id() == source {
                    return Err("Row merge would create a cycle".into());
                }
            }
            let children = document
                .tree
                .get(source)
                .ok_or("HTML row missing")?
                .children()
                .map(|c| (c.id(), c.value().is_fragment()))
                .collect::<Vec<_>>();
            charge(visits, children.len())?;
            if children.iter().any(|(_, fragment)| *fragment) {
                return Err("Template row merge unsupported".into());
            }
            let mut target = document.tree.get_mut(head).ok_or("HTML row missing")?;
            // Per-child append_id updates every parent link; reparent_from_id_append
            // only relinks the first and last child.
            for (child, _) in children {
                target.append_id(child);
            }
        }
        heads.push(chunk[0]);
    }
    // Moves add no nodes but can deepen the tree beyond the parse-time bound.
    check_document(document)?;
    Ok(heads)
}
fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn bounded(s: String) -> Result<String, String> {
    if s.len() > MAX_OUTPUT {
        Err("selector output limit exceeded".into())
    } else {
        Ok(s)
    }
}
pub(super) fn parse_css(s: &str) -> Result<Css, String> {
    check_css_bounds(s)?;
    Css::parse(s)
}
/// Length and `(` nesting bounds, also applied to [`Css::parse`]'s rewritten source.
pub(super) fn check_css_bounds(s: &str) -> Result<(), String> {
    if s.len() > MAX_SELECTOR {
        return Err("CSS selector length limit exceeded".into());
    }
    let mut depth = 0usize;
    for c in s.chars() {
        if c == '(' {
            depth += 1;
            if depth > 32 {
                return Err("CSS selector nesting limit exceeded".into());
            }
        }
        if c == ')' {
            depth = depth.saturating_sub(1);
        }
    }
    Ok(())
}
fn html_select<'a>(
    mut element: ElementRef<'a>,
    s: &str,
    include_self: bool,
) -> Result<Vec<ElementRef<'a>>, String> {
    // One :contains scan budget covers this whole selection pass.
    let scan = Scan::default();
    // handleSelector checks Matches before the special ancestor-root helper.
    if include_self && parse_css(s)?.matches(&element, &scan)? {
        return Ok(vec![element]);
    }
    let rooted = s.starts_with(":root");
    let s = if rooted {
        while let Some(p) = element.parent().and_then(ElementRef::wrap) {
            element = p;
        }
        s[5..].trim()
    } else {
        s
    };
    if s.is_empty() {
        return Err("empty CSS selector after :root expansion".into());
    }
    let selector = parse_css(s)?;
    let mut out = vec![];
    if include_self && !rooted && selector.matches(&element, &scan)? {
        return Ok(vec![element]);
    }
    for (count, e) in element
        .descendants()
        .filter_map(ElementRef::wrap)
        .enumerate()
    {
        if count >= MAX_VISITS {
            return Err("HTML selector visit limit exceeded".into());
        }
        if e.id() != element.id() && selector.matches(&e, &scan)? {
            out.push(e);
        }
    }
    Ok(std::mem::take(&mut out))
}
/// Whole-document node and depth bounds checked before any document-wide selection.
pub(super) fn check_document(document: &Html) -> Result<(), String> {
    for (count, node) in document.tree.nodes().enumerate() {
        if count >= MAX_VISITS {
            return Err("HTML document node limit exceeded".into());
        }
        if node.ancestors().take(130).count() > 128 {
            return Err("HTML document depth limit exceeded".into());
        }
    }
    Ok(())
}
/// AngleSharp `Document.QuerySelector(..) != null`: any element in document order,
/// the root element included, outside inert template contents. One `:contains`
/// budget covers the pass; callers check [`check_document`] first.
pub(super) fn document_matches(document: &Html, css: &Css) -> Result<bool, String> {
    document_first(document, css).map(|e| e.is_some())
}
/// AngleSharp `Document.QuerySelector`: the first matching element in document order.
pub(super) fn document_first<'a>(
    document: &'a Html,
    css: &Css,
) -> Result<Option<ElementRef<'a>>, String> {
    document_first_filtered(document, css, false)
}
/// Only for a parsed positive-local presence selector. Ignore the HTML select
/// element itself and its subtree: no parser-dependent element there proves login.
pub(super) fn document_local_presence(document: &Html, css: &Css) -> Result<bool, String> {
    debug_assert!(css.is_local_presence());
    document_first_filtered(document, css, true).map(|element| element.is_some())
}
pub(super) fn document_first_filtered<'a>(
    document: &'a Html,
    css: &Css,
    skip_select: bool,
) -> Result<Option<ElementRef<'a>>, String> {
    let scan = Scan::default();
    let root = document.root_element();
    let mut visits = 0usize;
    let mut next = Some(*root);
    while let Some(node) = next {
        // Template contents live under a Fragment child here, outside ChildNodes upstream.
        let inert = node.value().is_fragment()
            || (skip_select
                && node.value().as_element().is_some_and(|element| {
                    &*element.name.ns == "http://www.w3.org/1999/xhtml"
                        && element.name() == "select"
                }));
        if let Some(element) = ElementRef::wrap(node) {
            visits += 1;
            if visits > MAX_VISITS {
                return Err("HTML selector visit limit exceeded".into());
            }
            if !inert && css.matches(&element, &scan)? {
                return Ok(Some(element));
            }
        }
        next = if inert { None } else { node.first_child() };
        let mut cursor = node;
        while next.is_none() && cursor.id() != root.id() {
            next = cursor.next_sibling();
            match cursor.parent() {
                Some(parent) => cursor = parent,
                None => break,
            }
        }
    }
    Ok(None)
}
pub fn html_rows<'a>(
    document: &'a Html,
    selector: &str,
    max: usize,
) -> Result<Vec<ElementRef<'a>>, String> {
    check_document(document)?;
    let rows = html_select(document.root_element(), selector, false)?;
    if rows.len() > max {
        Err("HTML row limit exceeded".into())
    } else {
        Ok(rows)
    }
}
#[derive(Debug)]
struct JsonSelector {
    path: Vec<String>,
    filters: Vec<(String, JsonArgument)>,
}
#[derive(Debug)]
enum JsonArgument {
    Selector(Box<JsonSelector>),
    Text(String),
}
impl JsonSelector {
    fn parse(s: &str, depth: usize) -> Result<Self, String> {
        if s.len() > MAX_SELECTOR || depth > 16 {
            return Err("JSON selector complexity limit exceeded".into());
        }
        let split = s.find(':').unwrap_or(s.len());
        let path = parse_path(&s[..split])?;
        let mut rest = &s[split..];
        let mut filters = vec![];
        while !rest.is_empty() {
            let opening = rest.find('(').ok_or("JSON filter requires parentheses")?;
            let name = rest[1..opening].to_owned();
            let mut nesting = 1usize;
            let mut ending = None;
            for (i, c) in rest[opening + 1..].char_indices() {
                if c == '(' {
                    nesting += 1;
                }
                if c == ')' {
                    nesting -= 1;
                    if nesting == 0 {
                        ending = Some(opening + 1 + i);
                        break;
                    }
                }
            }
            let ending = ending.ok_or("unclosed JSON filter")?;
            let arg = &rest[opening + 1..ending];
            let arg = match name.as_str() {
                "has" | "not" => JsonArgument::Selector(Box::new(Self::parse(arg, depth + 1)?)),
                "contains" => JsonArgument::Text(arg.into()),
                _ => return Err(format!("unsupported JSON filter {name}")),
            };
            filters.push((name, arg));
            if filters.len() > 64 {
                return Err("JSON filter count limit exceeded".into());
            }
            rest = &rest[ending + 1..];
            if !rest.is_empty() && !rest.starts_with(':') {
                return Err("invalid JSON selector suffix".into());
            }
        }
        Ok(Self { path, filters })
    }
    fn select<'a>(&self, value: &'a Value) -> Result<Option<&'a Value>, String> {
        let mut selected = value;
        for k in &self.path {
            let next = match selected {
                Value::Array(a) => k.parse::<usize>().ok().and_then(|i| a.get(i)),
                Value::Object(o) => o.get(k),
                _ => None,
            };
            let Some(next) = next else {
                return Ok(None);
            };
            selected = next;
        }
        for (name, arg) in &self.filters {
            let matched = match arg {
                JsonArgument::Selector(s) => s.select(selected)?.is_some(),
                JsonArgument::Text(t) => json_string(selected)?.contains(t),
            };
            if (name == "not" && matched) || (name != "not" && !matched) {
                return Ok(None);
            }
        }
        Ok(Some(selected))
    }
}
fn parse_path(s: &str) -> Result<Vec<String>, String> {
    // Newtonsoft SelectToken's root is the token passed to the helper, including
    // a row when used for fields. Rooted compound paths remain outside this subset.
    if s == "$" {
        return Ok(vec![]);
    }
    let s = s.trim_start_matches('.');
    if s.is_empty() {
        return Ok(vec![]);
    }
    let mut path = vec![];
    let mut token = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '.' => {
                if !token.is_empty() {
                    path.push(std::mem::take(&mut token));
                }
            }
            '[' => {
                if !token.is_empty() {
                    path.push(std::mem::take(&mut token));
                }
                let mut bracket = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == ']' {
                        closed = true;
                        break;
                    }
                    bracket.push(c);
                }
                if !closed {
                    return Err("unclosed JSON bracket".into());
                }
                let key = bracket.trim_matches(['\'', '"']);
                if key.is_empty() || key.contains(['*', '?', ':', ',']) {
                    return Err("unsupported JSONPath expression".into());
                }
                path.push(key.into());
            }
            c if c.is_alphanumeric() || matches!(c, '_' | '-' | ' ' | '@') => token.push(c),
            _ => return Err(format!("unsupported JSONPath character {c}")),
        }
    }
    if !token.is_empty() {
        path.push(token);
    }
    if path.len() > 64 {
        return Err("JSON path depth limit exceeded".into());
    }
    Ok(path)
}
pub fn json_select<'a>(value: &'a Value, selector: &str) -> Result<Option<&'a Value>, String> {
    JsonSelector::parse(selector, 0)?.select(value)
}
pub fn json_rows<'a>(
    value: &'a Value,
    selector: &str,
    attribute: Option<&str>,
    max: usize,
) -> Result<Vec<&'a Value>, String> {
    if selector.is_empty() {
        return Err("empty JSON row selector".into());
    }
    let parsed = JsonSelector::parse(selector, 0)?;
    let base = JsonSelector {
        path: parsed.path,
        filters: vec![],
    };
    let Some(Value::Array(rows)) = base.select(value)? else {
        return Err("JSON rows selector does not resolve to array".into());
    };
    if rows.len() > MAX_VISITS {
        return Err("JSON row visit limit exceeded".into());
    }
    let filter = JsonSelector {
        path: vec![],
        filters: parsed.filters,
    };
    let mut out = vec![];
    for row in rows {
        if filter.select(row)?.is_some() {
            let row = if let Some(a) = attribute {
                json_select(row, a)?.ok_or("JSON row attribute missing")?
            } else {
                row
            };
            out.push(row);
            if out.len() > max {
                return Err("JSON row limit exceeded".into());
            }
        }
    }
    Ok(out)
}
fn json_string(value: &Value) -> Result<String, String> {
    bounded(match value {
        Value::String(s) => super::json_scalar::date_text(s)?.unwrap_or_else(|| s.clone()),
        Value::Bool(v) => if *v { "True" } else { "False" }.into(),
        Value::Null => String::new(),
        Value::Number(number) if number.is_f64() => {
            // Json.NET parses these as doubles; JValue.ToString() uses general
            // number formatting, not JSON's mandatory fractional '.0'. Keep the
            // stable ordinary-decimal subset; exponent/high-precision formatting
            // needs additional .NET compatibility work, not silent row dropping.
            let value = number.as_f64().ok_or("unsupported .NET JSON number")?;
            let text = value.to_string();
            let digits = text
                .chars()
                .filter(char::is_ascii_digit)
                .collect::<String>();
            if value.abs() >= 1e15
                || (value != 0.0 && value.abs() < 0.0001)
                || digits.trim_start_matches('0').len() > 15
                || text.contains(['e', 'E'])
            {
                return Err("unsupported .NET JSON floating-point formatting".into());
            }
            text
        }
        Value::Array(a) => a
            .iter()
            .map(json_string)
            .collect::<Result<Vec<_>, _>>()?
            .join(","),
        _ => serde_json::to_string_pretty(value).map_err(|e| e.to_string())?,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn compile(source: &str) -> Selector {
        Selector::compile(
            &crate::cardigann::parse(source, crate::cardigann::Limits::default()).unwrap(),
        )
        .unwrap()
    }
    #[test]
    fn nested_json_filters() {
        let v = serde_json::json!({"data":[{"attributes":{"name":"DTS 1080","size":12}},{"attributes":{"name":"720","size":3}}]});
        let rows = json_rows(
            &v,
            "data:has(attributes.size):has(attributes.name:contains(1080)):not(attributes.fake)",
            Some("attributes"),
            100,
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(json_select(rows[0], "name:not(:contains(DTS))")
            .unwrap()
            .is_none());
        assert!(json_select(rows[0], "name:contains(DTS)")
            .unwrap()
            .is_some());
    }
    #[test]
    fn unknown_and_limits() {
        assert!(json_select(&Value::Null, "a:unknown(x)").is_err());
        assert!(json_select(&Value::Null, "a[*]").is_err());
        assert!(JsonSelector::parse(&"a".repeat(5000), 0).is_err());
    }
    #[test]
    fn html_case_attribute_remove() {
        let html = Html::parse_document(
            "<div data-name='abc'><span class='badge'>bad</span> hello <b>world</b></div>",
        );
        let row = html_rows(&html, "div", 10).unwrap()[0];
        let id = |s: &str| Ok(s.to_owned());
        assert_eq!(
            compile("remove: .badge").extract_html(row, &id).unwrap(),
            Some("hello world".into())
        );
        assert_eq!(
            compile("case:\n  .badge: yes\n  '*': no")
                .extract_html(row, &id)
                .unwrap(),
            Some("yes".into())
        );
        assert_eq!(
            compile("selector: div\nattribute: data-name")
                .extract_html(row, &id)
                .unwrap(),
            Some("abc".into())
        );
    }
    #[test]
    fn html_attribute_names_fold_ascii_but_foreign_names_do_not() {
        let html = Html::parse_document(
            "<div onMouseOver='fixture' data-Ä='upper' data-ä='lower'></div><svg viewBox='0 0 1 1'></svg>",
        );
        let render = |s: &str| Ok(s.to_owned());
        let row = html_rows(&html, "div", 10).unwrap()[0];
        for name in ["onMouseOver", "onmouseover", "ONMOUSEOVER"] {
            assert_eq!(
                compile(&format!("attribute: {name}"))
                    .extract_html(row, &render)
                    .unwrap(),
                Some("fixture".into())
            );
        }
        for (name, expected) in [("data-Ä", "upper"), ("data-ä", "lower")] {
            assert_eq!(
                compile(&format!("attribute: {name}"))
                    .extract_html(row, &render)
                    .unwrap(),
                Some(expected.into())
            );
        }
        let foreign = html_rows(&html, "svg", 10).unwrap()[0];
        assert_eq!(
            compile("attribute: viewBox")
                .extract_html(foreign, &render)
                .unwrap(),
            Some("0 0 1 1".into())
        );
        assert_eq!(
            compile("attribute: viewbox")
                .extract_html(foreign, &render)
                .unwrap(),
            None
        );
    }
    #[test]
    fn json_cases_and_missing() {
        let id = |s: &str| Ok(s.to_owned());
        let s = compile("selector: value\ncase:\n  '0': one\n  '*': other");
        assert_eq!(
            s.extract_json(&serde_json::json!({"value":0}), &id)
                .unwrap(),
            Some("one".into())
        );
        assert_eq!(s.extract_json(&serde_json::json!({}), &id).unwrap(), None);
        assert_eq!(
            json_select(&serde_json::json!({"a":{"b":[4]}}), "a['b'][0]").unwrap(),
            Some(&serde_json::json!(4))
        );
    }
    #[test]
    fn removal_aggregate_budget_uses_rendered_root_and_no_rollback() {
        let mut html = Html::parse_document(
            "<main><p>outside</p><article><span>inside</span></article></main>",
        );
        let row = html_row_slots(&html, "article", 10).unwrap()[0];
        let selector = compile("selector: '{{ .Scope }}'\nremove: span\nattribute: missing");
        let mut visits = 1000;
        assert_eq!(
            selector
                .extract_html_mut(&mut html, row, &mut visits, &|_| Ok(":root article".into()))
                .unwrap(),
            None
        );
        let row_element = html_rows(&html, "article", 10).unwrap()[0];
        assert_eq!(
            compile("selector: span")
                .extract_html(row_element, &|s| Ok(s.into()))
                .unwrap(),
            None
        );
        let mut visits = row + 1; // Row lookup alone exhausts the budget.
        assert!(selector
            .extract_html_mut(&mut html, row, &mut visits, &|_| Ok(":root main".into()))
            .unwrap_err()
            .contains("aggregate"));
        let mut visits = 1000;
        assert!(compile("selector: ':root'")
            .extract_html_mut(&mut html, row, &mut visits, &|s| Ok(s.into()))
            .is_err());
    }
    #[test]
    fn html_depth() {
        let html =
            Html::parse_document(&format!("{}x{}", "<div>".repeat(140), "</div>".repeat(140)));
        assert!(html_rows(&html, "div", 1000).unwrap_err().contains("depth"));
    }
}
