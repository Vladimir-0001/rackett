//! JSON `rows.multiple`, parent `..` field selectors and `missingAttributeEqualsNoResults`
//! (pinned CardigannIndexer.cs:1681-1736) against synthetic responses.
use chrono::{TimeZone, Utc};
use rackett::{
    cardigann::{self, Limits},
    cardigann_exec::Plan,
    models::SearchQuery,
};
use std::collections::BTreeMap;
fn plan(rows: &str) -> Plan {
    let source = format!(
        r#"id: jsonrows
name: JSON rows
description: Synthetic
language: en-US
type: public
encoding: UTF-8
links: [https://example.org/]
caps:
  categories: {{Other: Other}}
  modes: {{search: [q]}}
search:
  paths: [{{path: api, response: {{type: json}}}}]
  inputs: {{q: '{{{{ .Keywords }}}}'}}
  rows:
{rows}
  fields:
    category: {{text: Other}}
    title: {{selector: ..title}}
    quality: {{selector: quality}}
    title_full: {{text: '{{{{ .Result.title }}}} {{{{ .Result.quality }}}}'}}
    download: {{selector: url}}
    size: {{text: 0}}
    seeders: {{text: 0}}
    leechers: {{text: 0}}
"#
    );
    Plan::compile(&cardigann::parse(&source, Limits::default()).unwrap()).unwrap()
}
fn extract(p: &Plan, body: &str) -> Result<Vec<(String, String)>, String> {
    let ctx = p.context(
        &BTreeMap::new(),
        &SearchQuery {
            params: BTreeMap::from([("q".into(), "x".into())]),
        },
        Utc.with_ymd_and_hms(2026, 1, 2, 12, 0, 0).unwrap(),
    );
    let request = p.build_requests(&ctx).unwrap().remove(0);
    p.extract(&request, body, &ctx)
        .map(|rows| rows.into_iter().map(|r| (r.title, r.link)).collect())
        .map_err(|e| format!("{}: {}", e.path, e.message))
}
const MULTIPLE: &str = "    selector: data.movies\n    attribute: torrents\n    multiple: true";
const MOVIES: &str = r#"{"data":{"movies":[
  {"title":"Alpha","url":"https://example.org/movie/a","torrents":[
    {"quality":"720p","url":"https://example.org/t/a720"},
    {"quality":"1080p","url":"https://example.org/t/a1080"}]},
  {"title":"Beta","url":"https://example.org/movie/b","torrents":[
    {"quality":"2160p","url":"https://example.org/t/b2160"}]}]}}"#;
#[test]
fn every_torrent_is_a_release_and_dotted_fields_read_the_movie() {
    let rows = extract(&plan(MULTIPLE), MOVIES).unwrap();
    assert_eq!(
        rows,
        vec![
            ("Alpha".into(), "https://example.org/t/a720".into()),
            ("Alpha".into(), "https://example.org/t/a1080".into()),
            ("Beta".into(), "https://example.org/t/b2160".into()),
        ]
    );
}
#[test]
fn missing_rows_or_attributes_are_errors_unless_flagged() {
    let without = r#"{"data":{"movie_count":0}}"#;
    assert!(extract(&plan(MULTIPLE), without)
        .unwrap_err()
        .starts_with("$.search.rows"));
    let flagged = plan(&format!(
        "{MULTIPLE}\n    missingAttributeEqualsNoResults: true"
    ));
    assert_eq!(extract(&flagged, without).unwrap(), vec![]);
    // A movie without torrents is skipped; the others still produce releases.
    let partial = r#"{"data":{"movies":[{"title":"Gone","url":"u"},
      {"title":"Kept","url":"u","torrents":[{"quality":"q","url":"https://example.org/t/k"}]}]}}"#;
    assert_eq!(
        extract(&flagged, partial).unwrap(),
        vec![("Kept".into(), "https://example.org/t/k".into())]
    );
    assert!(extract(&plan(MULTIPLE), partial).is_err());
}
#[test]
fn multiple_requires_an_array_of_objects() {
    let scalar = r#"{"data":{"movies":[{"title":"A","torrents":{"quality":"q","url":"u"}}]}}"#;
    assert!(extract(&plan(MULTIPLE), scalar)
        .unwrap_err()
        .contains("require an array"));
    let strings = r#"{"data":{"movies":[{"title":"A","torrents":["x"]}]}}"#;
    assert!(extract(&plan(MULTIPLE), strings)
        .unwrap_err()
        .contains("must be objects"));
}
#[test]
fn without_multiple_the_attribute_object_is_the_row_and_dots_read_its_parent() {
    let rows = "    selector: data.movies\n    attribute: torrent";
    let body = r#"{"data":{"movies":[{"title":"Solo","torrent":{"quality":"q","url":"https://example.org/t/s"}}]}}"#;
    assert_eq!(
        extract(&plan(rows), body).unwrap(),
        vec![("Solo".into(), "https://example.org/t/s".into())]
    );
}
#[test]
fn row_flags_must_be_booleans() {
    let source = |flag: &str| {
        format!(
            "id: x\nname: x\ndescription: x\nlanguage: en-US\ntype: public\nencoding: UTF-8\nlinks: [https://example.org/]\ncaps:\n  categories: {{Other: Other}}\n  modes: {{search: [q]}}\nsearch:\n  paths: [{{path: s, response: {{type: json}}}}]\n  rows:\n    selector: data\n    multiple: {flag}\n  fields:\n    title: {{selector: t}}\n    category: {{text: Other}}\n    download: {{text: /d}}\n    size: {{text: 0}}\n    seeders: {{text: 0}}\n    leechers: {{text: 0}}\n"
        )
    };
    let compiles = |flag: &str| {
        Plan::compile(&cardigann::parse(&source(flag), Limits::default()).unwrap()).is_ok()
    };
    assert!(compiles("true"));
    assert!(compiles("false"));
    assert!(!compiles("maybe"));
}
