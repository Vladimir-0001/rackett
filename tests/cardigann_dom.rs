//! Source: pinned CardigannIndexer.cs:1280–1369,1832–1854.
use chrono::{TimeZone, Utc};
use rackett::{
    cardigann::{self, Limits},
    cardigann_exec::Plan,
    models::SearchQuery,
};
use std::collections::BTreeMap;
fn compile(fields: &str) -> Plan {
    let source = format!(
        r#"id: dom
name: DOM
description: Synthetic
language: en-US
type: public
encoding: UTF-8
links: [https://example.org/]
caps:
  categories: {{Other: Other}}
  modes: {{search: [q]}}
search:
  paths: [{{path: search}}]
  rows: {{selector: 'article'}}
  fields:
    category: {{text: Other}}
    size: {{text: 0}}
    seeders: {{text: 0}}
    leechers: {{text: 0}}
    date: {{text: 2024-02-29}}
    download: {{text: /download}}
{fields}
"#
    );
    Plan::compile(&cardigann::parse(&source, Limits::default()).unwrap()).unwrap()
}
fn extract(p: &Plan, body: &str) -> Vec<rackett::models::Release> {
    let c = p.context(
        &BTreeMap::new(),
        &SearchQuery::default(),
        Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap(),
    );
    let request = p.build_requests(&c).unwrap().remove(0);
    p.extract(&request, body, &c).unwrap()
}
#[test]
fn ordered_remove_affects_cases_attributes_siblings_optional_and_append() {
    let p = compile(
        r#"    title: {selector: h2, remove: '.badge, .badge b'}
    title|append: {selector: h2, attribute: data-extra, filters: [{name: prepend, args: ' '}]}
    scratch|optional: {selector: section, remove: a, attribute: missing}
    description:
      selector: section
      case:
        'a': wrong
        '*': clean
    description|append: {selector: 'section > i:first-child', filters: [{name: prepend, args: ' '}]}
"#,
    );
    let body = "<article><h2 data-extra=' suffix'>Name<span class=badge>junk<b>nested</b></span></h2><section><a>remove</a><i>survived</i></section></article>";
    let r = extract(&p, body);
    assert_eq!(r[0].title, "Name suffix");
    assert_eq!(r[0].attributes["description"], "clean survived");
    assert_eq!(
        extract(&p, body)[0].attributes["description"],
        r[0].attributes["description"]
    );
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| assert_eq!(extract(&p, body)[0].title, "Name suffix"));
        }
    });
}
#[test]
fn text_bypasses_remove_and_remove_never_detaches_selection_itself() {
    let p = compile(
        r#"    scratch: {text: literal, remove: span}
    title: {selector: h2, remove: h2}
    description: {selector: span}
"#,
    );
    let r = extract(&p, "<article><h2>Name</h2><span>retained</span></article>");
    assert_eq!(r[0].title, "Name");
    assert_eq!(r[0].attributes["description"], "retained");
}
#[test]
fn nested_rows_keep_captured_detached_nodes_and_root_context_changes() {
    let p = compile(
        r#"    title: {selector: h2}
    scratch|optional: {selector: ':root section', remove: article}
    description: {selector: ':root h2'}
"#,
    );
    // Inner row is detached from section but remains a captured row. Its own
    // ancestor chain now ends at itself, so root selection sees its own h2.
    let r = extract(&p, "<section><article><h2>outer</h2><article><h2>inner</h2></article></article></section><h2>global</h2>");
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].attributes["description"], "outer");
    assert_eq!(r[1].attributes["description"], "inner");
}
#[test]
fn pinned_azusa_raw_request_and_table_dom_removal_keep_auth_blocked() {
    let node = cardigann::load(
        std::path::Path::new("vendor/jackett/definitions/azusa.yml"),
        Limits::default(),
    )
    .unwrap();
    let p = Plan::compile(&node).unwrap();
    assert!(!p.blockers().is_empty());
    let c = p.context(
        &BTreeMap::new(),
        &SearchQuery {
            params: BTreeMap::from([("q".into(), "comic".into()), ("cat".into(), "7030".into())]),
        },
        Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap(),
    );
    let request = p.build_requests(&c).unwrap().remove(0);
    assert!(request.url.contains("?cat402=1&"));
    let r = p
        .extract(
            &request,
            include_str!("fixtures/cardigann-semantics/azusa.html"),
            &c,
        )
        .unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].title, "Comic title");
    assert_eq!(r[0].attributes["description"], "A plain synopsis");
    assert_eq!(r[0].size, Some(1073741824));
    assert_eq!(r[0].link, "https://azusa.wiki/download.php?id=7");
    assert_eq!(r[0].categories, [7030, 100402]);
}
