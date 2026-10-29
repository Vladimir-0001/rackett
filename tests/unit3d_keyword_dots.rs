//! UNIT3D `keywords_replace_dots` keyword filter.
//!
//! Mirrors the pinned vendor/jackett/definitions/0dayfiles-api.yml
//! `search.keywordsfilters: re_replace ["\\.", " "]`, which upstream
//! CardigannIndexer applies to the joined `.Query.Keywords` before templating
//! `name: "{{ .Keywords }}"`. The loopback upstream records the raw request so
//! the tests prove the replacement happens before URL query encoding.
use axum::{
    extract::OriginalUri,
    http::{HeaderMap, StatusCode},
    Router,
};
use rackett::{
    adapters,
    models::{Indexer, SearchQuery},
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const DUMMY_KEY: &str = "dummy-unit3d-dots-key";

#[derive(Clone, Debug)]
struct Seen {
    path: String,
    raw_query: String,
    auth: String,
}

struct Upstream {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Upstream {
    fn drop(&mut self) {
        self.server.abort();
    }
}
async fn upstream() -> Upstream {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let app = Router::new().fallback(move |OriginalUri(uri): OriginalUri, headers: HeaderMap| {
        let log = log.clone();
        async move {
            log.lock().unwrap().push(Seen {
                path: uri.path().to_owned(),
                raw_query: uri.query().unwrap_or_default().to_owned(),
                auth: headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_owned(),
            });
            if uri.path().starts_with("/broken") {
                return (StatusCode::INTERNAL_SERVER_ERROR, String::new());
            }
            let body = json!({"data":[{"id":"1","attributes":{
                "name":"Dotted.Release.Name","download_link":"/download/1"}}]});
            (StatusCode::OK, body.to_string())
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Upstream {
        base: format!("http://{addr}"),
        seen,
        server,
    }
}

fn unit3d(url: &str, definition: Option<Value>) -> Indexer {
    Indexer {
        id: "unit3d-dots".into(),
        name: "UNIT3D dots fixture".into(),
        kind: "unit3d".into(),
        url: url.into(),
        api_key: DUMMY_KEY.into(),
        request_delay_ms: 0,
        definition,
        ..Default::default()
    }
}
fn query(pairs: &[(&str, &str)]) -> SearchQuery {
    SearchQuery {
        params: pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}
fn pairs(raw: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(raw.as_bytes())
        .into_owned()
        .collect()
}

/// Search once and return the single recorded request.
async fn send(up: &Upstream, url: &str, definition: Option<Value>, q: &SearchQuery) -> Seen {
    let before = up.seen.lock().unwrap().len();
    let releases = adapters::search(&reqwest::Client::new(), &unit3d(url, definition), q)
        .await
        .unwrap();
    // Response titles are never rewritten: the filter only touches the outgoing keywords.
    assert_eq!(releases[0].title, "Dotted.Release.Name");
    let seen = up.seen.lock().unwrap();
    assert_eq!(seen.len(), before + 1);
    let request = seen[before].clone();
    assert_eq!(request.auth, format!("Bearer {DUMMY_KEY}"));
    assert!(!request.raw_query.contains(DUMMY_KEY));
    request
}
fn name_of(seen: &Seen) -> String {
    let names: Vec<_> = pairs(&seen.raw_query)
        .into_iter()
        .filter(|(k, _)| k == "name")
        .map(|(_, v)| v)
        .collect();
    assert_eq!(names.len(), 1, "{}", seen.raw_query);
    names[0].clone()
}

fn enabled() -> Option<Value> {
    Some(json!({"keywords_replace_dots": true}))
}

/// (q, decoded name when disabled, decoded name when enabled, raw `name=` when enabled)
const CASES: &[(&str, &str, &str, &str)] = &[
    (
        "Some.Show.2024",
        "Some.Show.2024",
        "Some Show 2024",
        "name=Some+Show+2024",
    ),
    ("a..b...c", "a..b...c", "a  b   c", "name=a++b+++c"),
    // Keywords are trimmed before filtering (as before); the result is not.
    (
        "..lead.trail..",
        "..lead.trail..",
        "  lead trail  ",
        "name=++lead+trail++",
    ),
    (".", ".", " ", "name=+"),
    ("  a . b\t.c  ", "a . b\t.c", "a   b\t c", "name=a+++b%09+c"),
    (
        "already spaced",
        "already spaced",
        "already spaced",
        "name=already+spaced",
    ),
    (
        "100%.a&b+c.d",
        "100%.a&b+c.d",
        "100% a&b+c d",
        "name=100%25+a%26b%2Bc+d",
    ),
    // Only ASCII '.' is replaced; non-ASCII lookalikes and other text stay intact.
    (
        "Caf\u{e9}.\u{2024}\u{3002}\u{ff0e}\u{b7}x",
        "Caf\u{e9}.\u{2024}\u{3002}\u{ff0e}\u{b7}x",
        "Caf\u{e9} \u{2024}\u{3002}\u{ff0e}\u{b7}x",
        "name=Caf%C3%A9+%E2%80%A4%E3%80%82%EF%BC%8E%C2%B7x",
    ),
    ("", "", "", "name="),
];

#[tokio::test]
async fn disabled_keeps_dots_and_enabled_replaces_each_before_encoding() {
    let up = upstream().await;
    let url = format!("{}/", up.base);
    for (q, off, on, raw_on) in CASES {
        let q = query(&[("q", q)]);
        for definition in [
            None,
            Some(json!({})),
            Some(json!({"keywords_replace_dots": false})),
        ] {
            let seen = send(&up, &url, definition, &q).await;
            assert_eq!(name_of(&seen), *off);
        }
        let seen = send(&up, &url, enabled(), &q).await;
        assert_eq!(seen.path, "/api/torrents/filter");
        assert_eq!(name_of(&seen), *on);
        assert!(
            seen.raw_query.starts_with(&format!("{raw_on}&")),
            "{}",
            seen.raw_query
        );
    }
}

#[tokio::test]
async fn disabled_request_is_byte_identical_to_absent_option() {
    let up = upstream().await;
    let url = format!("{}/", up.base);
    let q = query(&[("q", "Some.Show"), ("season", "1"), ("ep", "2")]);
    let absent = send(&up, &url, None, &q).await;
    let empty = send(&up, &url, Some(json!({})), &q).await;
    let off = send(&up, &url, Some(json!({"keywords_replace_dots": false})), &q).await;
    assert_eq!(absent.raw_query, empty.raw_query);
    assert_eq!(absent.raw_query, off.raw_query);
    assert_eq!(
        absent.raw_query,
        "name=Some.Show+S01E02&seasonNumber=1&episodeNumber=2&perPage=100&page=1\
         &sortField=created_at&sortDirection=desc"
    );
}

#[tokio::test]
async fn only_name_changes_ids_categories_paging_and_base_url_are_untouched() {
    let up = upstream().await;
    // Dots in the host (127.0.0.1), base path and a pre-existing base query stay as configured.
    let url = format!("{}/tracker.v2/?keep=a.b", up.base);
    let q = query(&[
        ("q", "Some.Show"),
        ("season", "1"),
        ("ep", "2"),
        ("imdbid", "tt0123456"),
        ("tmdbid", "12.5"),
        ("tvdbid", "7.1"),
        ("cat", "2000,5000"),
        ("limit", "50"),
        ("offset", "100"),
    ]);
    let categories = json!({"2000": [7, "9"], "5000": [8]});
    let off = send(&up, &url, Some(json!({"categories": categories})), &q).await;
    let on = send(
        &up,
        &url,
        Some(json!({
            "categories": categories,
            "keywords_replace_dots": true,
            "single_file_release_use_filename": true,
        })),
        &q,
    )
    .await;
    assert_eq!(off.path, "/tracker.v2/api/torrents/filter");
    assert_eq!(on.path, off.path);
    let expected = |name: &str| {
        [
            ("keep", "a.b"),
            ("name", name),
            ("seasonNumber", "1"),
            ("episodeNumber", "2"),
            ("imdbId", "0123456"),
            ("tmdbId", "12.5"),
            ("tvdbId", "7.1"),
            ("perPage", "50"),
            ("page", "3"),
            ("sortField", "created_at"),
            ("sortDirection", "desc"),
            ("categories[]", "7"),
            ("categories[]", "9"),
            ("categories[]", "8"),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .to_vec()
    };
    assert_eq!(pairs(&off.raw_query), expected("Some.Show S01E02"));
    assert_eq!(pairs(&on.raw_query), expected("Some Show S01E02"));
    assert_eq!(
        on.raw_query,
        off.raw_query
            .replace("name=Some.Show+S01E02", "name=Some+Show+S01E02")
    );
}

#[tokio::test]
async fn season_episode_and_no_query_keep_search_query_handling() {
    let up = upstream().await;
    let url = format!("{}/", up.base);
    // Season without q: keywords are the episode token only.
    let seen = send(
        &up,
        &url,
        enabled(),
        &query(&[("season", "3"), ("ep", "4")]),
    )
    .await;
    assert_eq!(name_of(&seen), "S03E04");
    // Dotted multi-field keywords join first, then each ASCII dot is replaced.
    let seen = send(
        &up,
        &url,
        enabled(),
        &query(&[("q", "A.B"), ("season", "1"), ("title", "x.y")]),
    )
    .await;
    assert_eq!(name_of(&seen), "A B S01 x y");
    // No query at all.
    let seen = send(&up, &url, enabled(), &SearchQuery::default()).await;
    assert!(seen.raw_query.starts_with("name=&perPage=100&page=1&"));
}

#[tokio::test]
async fn upstream_error_does_not_leak_key_or_query() {
    let up = upstream().await;
    let url = format!("{}/broken/", up.base);
    let err = adapters::search(
        &reqwest::Client::new(),
        &unit3d(&url, enabled()),
        &query(&[("q", "Secret.Dotted")]),
    )
    .await
    .unwrap_err();
    let text = format!("{err:#}");
    assert!(!text.contains(DUMMY_KEY), "{text}");
    assert!(!text.contains("Secret"), "{text}");
    let seen = up.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].auth, format!("Bearer {DUMMY_KEY}"));
    assert!(seen[0].raw_query.starts_with("name=Secret+Dotted&"));
    assert!(!seen[0].raw_query.contains(DUMMY_KEY));
}

#[test]
fn option_is_strict_boolean_unit3d_only_and_unknown_fields_stay_rejected() {
    let ok =
        |definition: Value| adapters::validate(&unit3d("http://127.0.0.1:9/", Some(definition)));
    assert!(ok(json!({"keywords_replace_dots": true})).is_ok());
    assert!(ok(json!({"keywords_replace_dots": false})).is_ok());
    assert!(ok(json!({
        "keywords_replace_dots": true,
        "single_file_release_use_filename": true,
        "categories": {"2000": [1]},
    }))
    .is_ok());
    for bad in [
        json!("true"),
        json!("false"),
        json!(null),
        json!(1),
        json!(0),
        json!([true]),
        json!({"value": true}),
    ] {
        let err = ok(json!({"keywords_replace_dots": bad})).unwrap_err();
        assert!(err.to_string().contains("must be a boolean"), "{err}");
    }
    for unknown in [
        json!({"keyword_replace_dots": true}),
        json!({"keywordsfilters": [{"name": "re_replace", "args": ["\\.", " "]}]}),
    ] {
        assert!(ok(unknown)
            .unwrap_err()
            .to_string()
            .contains("Unknown adapter definition field"));
    }
    for kind in ["rss", "torznab", "json", "html"] {
        let mut other = unit3d(
            "http://127.0.0.1:9/",
            Some(json!({"keywords_replace_dots": true})),
        );
        other.kind = kind.into();
        assert!(adapters::validate(&other).is_err(), "{kind}");
    }
}

#[tokio::test]
async fn other_kinds_keep_dotted_keywords() {
    let up = upstream().await;
    for kind in ["torznab", "rss"] {
        let mut indexer = unit3d(&format!("{}/feed.xml", up.base), None);
        indexer.kind = kind.into();
        // The fixture body is not a feed; only the outgoing request matters here.
        let _ = adapters::search(
            &reqwest::Client::new(),
            &indexer,
            &query(&[("q", "Some.Show")]),
        )
        .await;
    }
    let seen = up.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(seen[0].raw_query.contains("q=Some.Show"), "{:?}", seen[0]);
    assert_eq!(seen[1].raw_query, "q=Some.Show");
}
