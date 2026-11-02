//! `rows.dateheaders` (pinned CardigannIndexer.cs:1910-1946) against synthetic pages.
use chrono::{TimeZone, Utc};
use rackett::{
    cardigann::{self, Limits},
    cardigann_exec::Plan,
    models::SearchQuery,
};
use std::collections::BTreeMap;
fn plan(rows: &str, date: &str) -> Plan {
    let source = format!(
        r#"id: dateheaders
name: Date headers
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
  inputs: {{q: '{{{{ .Keywords }}}}'}}
  rows:
{rows}
  fields:
    category: {{text: Other}}
    title: {{selector: a}}
    download: {{text: /download}}
    size: {{text: 0}}
    seeders: {{text: 0}}
    leechers: {{text: 0}}
{date}"#
    );
    Plan::compile(&cardigann::parse(&source, Limits::default()).unwrap()).unwrap()
}
fn extract(p: &Plan, body: &str) -> Result<Vec<(String, Option<String>)>, String> {
    let ctx = p.context(
        &BTreeMap::new(),
        &SearchQuery {
            params: BTreeMap::from([("q".into(), "x".into())]),
        },
        Utc.with_ymd_and_hms(2026, 1, 2, 12, 0, 0).unwrap(),
    );
    let request = p.build_requests(&ctx).unwrap().remove(0);
    p.extract(&request, body, &ctx)
        .map(|rows| rows.into_iter().map(|r| (r.title, r.published)).collect())
        .map_err(|e| format!("{}: {}", e.path, e.message))
}
const ROWS: &str = r#"    selector: tr.t
    dateheaders:
      selector: td.d
      filters:
        - name: replace
          args: ["Added ", ""]"#;
#[test]
fn undated_rows_take_the_nearest_earlier_header() {
    let p = plan(ROWS, "");
    let page = r#"<table>
<tr><td class="d">Added 2024-03-01</td></tr>
<tr class="t"><td><a>One</a></td></tr>
<tr class="t"><td><a>Two</a></td></tr>
<tr><td class="d">Added 2024-02-28</td></tr>
<tr class="t"><td><a>Three</a></td></tr>
</table>"#;
    let rows = extract(&p, page).unwrap();
    let date = |d: &str| Some(format!("{d}T00:00:00Z"));
    assert_eq!(
        rows,
        vec![
            ("One".into(), date("2024-03-01")),
            ("Two".into(), date("2024-03-01")),
            ("Three".into(), date("2024-02-28")),
        ]
    );
}
#[test]
fn walk_continues_at_the_parents_previous_sibling() {
    // The row has no previous sibling, so upstream moves to its parent's previous sibling:
    // the tbody's is the header tbody, whose descendant matches.
    let page = r#"<table><tbody><tr><td class="d">Added 2024-01-05</td></tr></tbody>
<tbody><tr class="t"><td><a>Nested</a></td></tr></tbody></table>"#;
    let rows = extract(&plan(ROWS, ""), page).unwrap();
    assert_eq!(rows[0].1.as_deref(), Some("2024-01-05T00:00:00Z"));
    // handleSelector tests the element itself before its descendants, so a header row that
    // matches the selector (as cyclingarchiveclub's `:has(...)` does) is found.
    let own = r#"<table><tr><td>2024-01-05</td></tr><tr class="t"><td><a>A</a></td></tr></table>"#;
    let p = plan(&ROWS.replace("td.d", "tr:not(.t)"), "");
    assert_eq!(
        extract(&p, own).unwrap()[0].1.as_deref(),
        Some("2024-01-05T00:00:00Z")
    );
}
#[test]
fn rows_with_their_own_date_keep_it_and_missing_headers_fail_unless_optional() {
    let p = plan(ROWS, "    date: {text: 2020-05-06}\n");
    let page = r#"<table><tr><td class="d">Added 2024-03-01</td></tr><tr class="t"><td><a>A</a></td></tr></table>"#;
    assert_eq!(
        extract(&p, page).unwrap()[0].1.as_deref(),
        Some("2020-05-06T00:00:00Z")
    );
    let headerless = r#"<table><tr class="t"><td><a>A</a></td></tr></table>"#;
    let err = extract(&plan(ROWS, ""), headerless).unwrap_err();
    assert!(err.starts_with("$.search.rows.dateheaders"), "{err}");
    let optional = plan(&format!("{ROWS}\n      optional: true"), "");
    assert_eq!(extract(&optional, headerless).unwrap()[0].1, None);
}
#[test]
fn the_nearest_match_wins_even_when_its_date_does_not_parse() {
    // A selector miss walks on; a matched value is final. `dateparse` returns unparsable
    // input unchanged, as upstream does, and the date conversion then fails the row.
    let rows = r#"    selector: tr.t
    dateheaders:
      selector: td.d
      filters:
        - name: dateparse
          args: "yyyy-MM-dd""#;
    let page = r#"<table>
<tr><td class="d">2024-04-02</td></tr>
<tr><td>no header here</td></tr>
<tr class="t"><td><a>A</a></td></tr>
</table>"#;
    assert_eq!(
        extract(&plan(rows, ""), page).unwrap()[0].1.as_deref(),
        Some("2024-04-02T00:00:00Z")
    );
    let unparsable = page.replace("<td>no header here</td>", r#"<td class="d">soon</td>"#);
    let err = extract(&plan(rows, ""), &unparsable).unwrap_err();
    assert!(err.starts_with("$.search.rows.dateheaders"), "{err}");
}
#[test]
fn unsupported_header_keys_are_rejected() {
    let source = |extra: &str| {
        format!(
            "id: x\nname: x\ndescription: x\nlanguage: en-US\ntype: public\nencoding: UTF-8\nlinks: [https://example.org/]\ncaps:\n  categories: {{Other: Other}}\n  modes: {{search: [q]}}\nsearch:\n  paths: [{{path: s}}]\n  rows:\n    selector: tr\n    dateheaders: {{selector: td, {extra}}}\n  fields:\n    title: {{selector: a}}\n    category: {{text: Other}}\n    download: {{text: /d}}\n    size: {{text: 0}}\n    seeders: {{text: 0}}\n    leechers: {{text: 0}}\n"
        )
    };
    let compile = |extra: &str| {
        Plan::compile(&cardigann::parse(&source(extra), Limits::default()).unwrap())
            .map_err(|e| eprintln!("{extra}: {}: {}", e.path, e.message))
            .is_ok()
    };
    assert!(compile("attribute: title"));
    assert!(!compile("remove: b"));
}
