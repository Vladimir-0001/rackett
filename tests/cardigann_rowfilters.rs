//! Source: Jackett fe8876d, CardigannIndexer.cs:2435 and TorznabQuery.cs:217.
use chrono::Utc;
use rackett::{
    cardigann::{self, Limits},
    cardigann_exec::Plan,
    models::SearchQuery,
};
use std::collections::BTreeMap;
fn source(filters: &str, caps: &str, pipeline: &str) -> String {
    format!(
        r#"id: rowfilters
name: Rowfilters
description: Test
language: en-US
type: public
encoding: UTF-8
links: [https://example.org/]
caps:
  categories: {{Other: Other}}
  modes: {{search: [q], {caps}}}
search:
  paths: [{{path: search}}]
{pipeline}
  rows:
    selector: article
    filters: {filters}
  fields:
    category: {{text: Other}}
    size: {{text: 0}}
    seeders: {{text: 0}}
    leechers: {{text: 0}}
    date: {{text: 2024-02-29}}
    title: {{selector: span}}
    title|append: {{text: ' FINAL'}}
    download: {{text: /download}}
"#
    )
}
fn run(source: &str, q: &str, title: &str, id: Option<(&str, &str)>) -> Result<usize, String> {
    let p = Plan::compile(&cardigann::parse(source, Limits::default()).unwrap())
        .map_err(|e| e.to_string())?;
    let mut params = BTreeMap::from([("q".into(), q.into())]);
    if let Some((k, v)) = id {
        params.insert(k.into(), v.into());
    }
    let c = p.context(&BTreeMap::new(), &SearchQuery { params }, Utc::now());
    let r = p.build_requests(&c).map_err(|e| e.to_string())?;
    p.extract(
        &r[0],
        &format!("<article><span>{title}</span></article>"),
        &c,
    )
    .map(|r| r.len())
    .map_err(|e| e.to_string())
}
#[test]
fn andmatch_all_fields_common_words_substrings_unicode_and_empty() {
    let s = source("[{name: andmatch}]", "", "");
    for (query, title, want) in [
        ("THE an and x", "anything", 1),
        ("alpha beta", "BETA alphabet", 1),
        ("alpha beta", "alpha", 0),
        ("café", "CAFÉ", 1),
        ("café", "CAFE", 1),
        ("東京", "東京", 1),
        ("final", "title", 1),
        ("", "title", 1),
        ("under_score", "under score", 0),
    ] {
        assert_eq!(
            run(&s, query, title, None).unwrap(),
            want,
            "{query} / {title}"
        );
    }
}
#[test]
fn character_limit_and_first_filter_cache_use_utf16() {
    let s = source(
        "[{name: andmatch, args: 5}, {name: andmatch, args: 200}]",
        "",
        "",
    );
    assert_eq!(run(&s, "alpha missing", "alpha", None).unwrap(), 1);
    let s = source("[{name: andmatch, args: 3}]", "", "");
    assert_eq!(run(&s, "𐐀 missing", "none", None).unwrap(), 1); // .NET regex sees surrogate halves, neither is a word letter
    let s = source("[{name: andmatch, args: 1}]", "", "");
    assert_eq!(run(&s, "𐐀 missing", "none", None).unwrap(), 1); // split surrogate discarded
}
#[test]
fn transformed_keywords_and_whitespace_fallback() {
    let s = source(
        "[{name: andmatch}]",
        "",
        "  keywordsfilters: [{name: replace, args: [original, processed]}]",
    );
    assert_eq!(run(&s, "original", "processed", None).unwrap(), 1);
    assert_eq!(run(&s, "original", "original", None).unwrap(), 0);
    let s = source(
        "[{name: andmatch}]",
        "",
        "  keywordsfilters: [{name: replace, args: [original, ' ']}]",
    );
    assert_eq!(run(&s, "original", "wrong", None).unwrap(), 0);
}
#[test]
fn id_bypass_uses_actual_capabilities_including_upstream_quirks() {
    let no = source("[{name: andmatch}]", "", "");
    assert_eq!(
        run(&no, "missing", "wrong", Some(("imdbid", "tt123"))).unwrap(),
        0
    );
    let imdb = source("[{name: andmatch}]", "movie-search: [q, imdbid]", "");
    for key in ["imdbid", "doubanid", "traktid"] {
        assert_eq!(
            run(&imdb, "missing", "wrong", Some((key, "123"))).unwrap(),
            1
        );
    }
    assert_eq!(
        run(&imdb, "missing", "wrong", Some(("tvmazeid", "123"))).unwrap(),
        0
    );
    let tv = source("[{name: andmatch}]", "tv-search: [q, tvdbid, imdbid]", "");
    assert_eq!(
        run(&tv, "missing", "wrong", Some(("tvdbid", "123"))).unwrap(),
        1
    );
    assert_eq!(
        run(&tv, "missing", "wrong", Some(("imdbid", "123"))).unwrap(),
        0
    );
}
#[test]
fn unknown_debug_grammar_and_work_bounds_are_explicit() {
    for filters in [
        "[{name: other}]",
        "[{name: strdump}]",
        "[{name: andmatch, args: [5]}]",
        "[{name: andmatch, args: 2147483648}]",
    ] {
        assert!(run(&source(filters, "", ""), "q", "q", None)
            .unwrap_err()
            .contains("$.search.rows.filters[0]"));
    }
    assert!(run(
        &source("[{name: andmatch}]", "", ""),
        &"word ".repeat(4097),
        "word",
        None
    )
    .unwrap_err()
    .contains("token limit"));
    assert!(run(
        &source("[{name: andmatch}]", "", ""),
        "straße",
        "strasse",
        None
    )
    .unwrap_err()
    .contains("collation"));
}
#[test]
fn pinned_vstorrent_definition_andmatch_retains_download_blocker() {
    let source = include_str!("../vendor/jackett/definitions/vstorrent.yml");
    let p = Plan::compile(&cardigann::parse(source, Limits::default()).unwrap()).unwrap();
    assert!(p
        .blockers()
        .iter()
        .any(|b| b.feature == "download-resolution"));
    let c = p.context(
        &BTreeMap::new(),
        &SearchQuery {
            params: BTreeMap::from([("q".into(), "the piano".into())]),
        },
        Utc::now(),
    );
    let req = p.build_requests(&c).unwrap();
    let rows = p
        .extract(
            &req[0],
            include_str!("fixtures/cardigann-rowfilters/vstorrent.html"),
            &c,
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "Grand Piano Bundle");
    assert_eq!(rows[0].link, "https://vstorrent.org/piano/");
}

#[test]
fn whitespace_processed_fallback_sanitizes_query_punctuation() {
    let s = source(
        "[{name: andmatch}]",
        "",
        "  keywordsfilters: [{name: replace, args: ['foo&bar', ' ']}]",
    );
    assert_eq!(run(&s, "foo&bar", "foobar", None).unwrap(), 1);
    assert_eq!(run(&s, "foo&bar", "foo bar", None).unwrap(), 0);
}
#[test]
fn json_rows_receive_identical_post_field_filters() {
    let s = source("[{name: andmatch}]", "", "")
        .replace(
            "paths: [{path: search}]",
            "paths: [{path: search, response: {type: json}}]",
        )
        .replace("selector: article", "selector: data");
    let p = Plan::compile(&cardigann::parse(&s, Limits::default()).unwrap()).unwrap();
    let c = p.context(
        &BTreeMap::new(),
        &SearchQuery {
            params: BTreeMap::from([("q".into(), "alpha FINAL".into())]),
        },
        Utc::now(),
    );
    let req = p.build_requests(&c).unwrap();
    let rows = p
        .extract(
            &req[0],
            r#"{"data":[{"span":"alpha"},{"span":"beta"}]}"#,
            &c,
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "alpha FINAL");
}
