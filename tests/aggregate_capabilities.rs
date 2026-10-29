//! `/torznab/all` dispatches only to sources that can handle the query, like
//! Jackett's aggregate `indexers.Where(i => i.CanHandleQuery(query))`.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode, Uri},
    response::Html,
    routing::get,
    Router,
};
use rackett::{engine::Engine, models::Indexer, AppState};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const ACGRIP: &str = "<table><tbody><tr><td class=title><span class=title><a href='/details/1'>Fixture Anime</a></span></td><td class=action><a href='/file.torrent'>Get</a></td><td class=size>1 GB</td><td class=date><time datetime='2026-10-27T00:00:00Z'></time></td></tr></tbody></table>";
const FEED: &str = "<rss version=\"2.0\"><channel><item><title>Fixture Feed</title><guid>urn:fixture:feed</guid><pubDate>Mon, 26 Oct 2026 00:00:00 +0000</pubDate><enclosure url=\"/torrent\" length=\"42\" type=\"application/x-bittorrent\"/></item></channel></rss>";
const TORZNAB: &str = "<rss version=\"2.0\"><channel><item><title>Fixture Torznab</title><guid>urn:fixture:torznab</guid><pubDate>Sun, 25 Oct 2026 00:00:00 +0000</pubDate><enclosure url=\"/torrent\" length=\"42\" type=\"application/x-bittorrent\"/></item></channel></rss>";

type Hits = Arc<Mutex<Vec<String>>>;
struct Harness {
    app: Router,
    engine: Arc<Engine>,
    key: String,
    hits: Hits,
    ids: [String; 4],
    _dir: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
    }
}
fn record(hits: &Hits, name: &str, uri: &Uri) {
    hits.lock()
        .unwrap()
        .push(format!("{name}?{}", uri.query().unwrap_or("")));
}
impl Harness {
    /// Mixed capabilities: a ready TV-only bundled source (acgrip), a generic feed,
    /// a Torznab source and a UNIT3D source that fails if it is ever contacted.
    async fn new() -> Self {
        let hits: Hits = Arc::default();
        let (a, f, t, u) = (hits.clone(), hits.clone(), hits.clone(), hits.clone());
        let upstream = Router::new()
            .route(
                "/",
                get(move |uri: Uri| async move {
                    record(&a, "acgrip", &uri);
                    Html(ACGRIP)
                }),
            )
            .route(
                "/feed",
                get(move |uri: Uri| async move {
                    record(&f, "feed", &uri);
                    FEED
                }),
            )
            .route(
                "/torznab",
                get(move |uri: Uri| async move {
                    record(&t, "torznab", &uri);
                    TORZNAB
                }),
            )
            .route(
                "/unit3d/api/torrents/filter",
                get(move |uri: Uri| async move {
                    record(&u, "unit3d", &uri);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Unexpected UNIT3D request",
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let engine = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
        let mut ids = Vec::new();
        for source in [
            Indexer {
                name: "Fixture acgrip".into(),
                kind: "cardigann".into(),
                url: format!("http://{addr}/"),
                definition: Some(json!({"builtin_id":"acgrip","settings":{}})),
                ..Default::default()
            },
            Indexer {
                name: "Fixture feed".into(),
                url: format!("http://{addr}/feed"),
                categories: vec![2000],
                ..Default::default()
            },
            Indexer {
                name: "Fixture torznab".into(),
                kind: "torznab".into(),
                url: format!("http://{addr}/torznab"),
                api_key: "fixture-key".into(),
                categories: vec![2000, 5000],
                ..Default::default()
            },
            Indexer {
                name: "Fixture unit3d".into(),
                kind: "unit3d".into(),
                url: format!("http://{addr}/unit3d"),
                api_key: "fixture-key".into(),
                ..Default::default()
            },
        ] {
            let source = Indexer {
                request_delay_ms: 0,
                ..source
            };
            ids.push(engine.put_indexer(source, false).await.unwrap().id);
        }
        let key = engine.settings().await.api_key;
        let app = Router::new()
            .merge(rackett::protocol::router())
            .with_state(AppState {
                engine: engine.clone(),
            });
        Self {
            app,
            engine,
            key,
            hits,
            ids: ids.try_into().unwrap(),
            _dir: dir,
            server,
        }
    }
    async fn get(&self, id: &str, query: &str) -> (StatusCode, String) {
        let path = format!("/torznab/{id}/api?apikey={}&{query}", self.key);
        self.raw(&path).await
    }
    async fn raw(&self, path: &str) -> (StatusCode, String) {
        let response = self
            .app
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }
    async fn disable(&self, id: &str) {
        let mut source = self.engine.indexer(id).await.unwrap();
        source.enabled = false;
        self.engine.put_indexer(source, true).await.unwrap();
    }
    /// Upstream sources contacted since the last call, in name order.
    fn take_hits(&self) -> Vec<String> {
        let mut hits = std::mem::take(&mut *self.hits.lock().unwrap());
        hits.sort();
        hits
    }
    fn contacted(&self) -> Vec<String> {
        let mut names: Vec<_> = self
            .take_hits()
            .into_iter()
            .map(|h| h.split('?').next().unwrap().to_owned())
            .collect();
        names.dedup();
        names
    }
}
fn titles(body: &str) -> Vec<String> {
    let doc = roxmltree::Document::parse(body).unwrap();
    assert_eq!(doc.root_element().tag_name().name(), "rss", "{body}");
    doc.descendants()
        .filter(|n| n.tag_name().name() == "item")
        .map(|item| {
            item.children()
                .find(|n| n.tag_name().name() == "title")
                .and_then(|n| n.text())
                .unwrap()
                .to_owned()
        })
        .collect()
}
fn total(body: &str) -> (String, String) {
    let doc = roxmltree::Document::parse(body).unwrap();
    let r = doc
        .descendants()
        .find(|n| n.tag_name().name() == "response")
        .unwrap();
    (
        r.attribute("offset").unwrap().into(),
        r.attribute("total").unwrap().into(),
    )
}
fn error_code(body: &str) -> String {
    let doc = roxmltree::Document::parse(body).unwrap();
    assert_eq!(doc.root_element().tag_name().name(), "error", "{body}");
    doc.root_element().attribute("code").unwrap().into()
}

#[tokio::test]
async fn aggregate_movie_search_skips_tv_only_source() {
    for t in ["movie", "MOVIE", "Movie"] {
        // A fresh engine per case keeps the search cache from hiding dispatch.
        let h = Harness::new().await;
        let (status, body) = h.get("all", &format!("t={t}&q=fixture")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(titles(&body), ["Fixture Feed", "Fixture Torznab"]);
        // The UNIT3D fixture fails, but compatible results still merge.
        assert_eq!(h.contacted(), ["feed", "torznab", "unit3d"], "t={t}");
    }
}

#[tokio::test]
async fn explicit_incompatible_source_remains_an_error() {
    let h = Harness::new().await;
    for t in ["movie", "MOVIE"] {
        let (status, body) = h.get(&h.ids[0], &format!("t={t}&q=fixture")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error_code(&body), "203");
    }
    let (status, body) = h.get(&h.ids[1], "t=movie&imdbid=tt1234567").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "203");
    let (status, body) = h.get(&h.ids[3], "t=tvsearch&tvmazeid=42").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_code(&body), "203");
    assert!(h.take_hits().is_empty());
}

#[tokio::test]
async fn aggregate_identifier_search_uses_only_sources_supporting_the_identifier() {
    let h = Harness::new().await;
    let (status, body) = h.get("all", "t=tvsearch&tvmazeid=42&cat=5000").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(titles(&body), ["Fixture Torznab"]);
    let hits = h.take_hits();
    assert_eq!(hits.len(), 1, "{hits:?}");
    assert!(hits[0].starts_with("torznab?"));
    assert!(hits[0].contains("tvmazeid=42"), "{hits:?}");
    assert!(hits[0].contains("cat=5000"), "{hits:?}");
    assert!(hits[0].contains("t=tvsearch"), "{hits:?}");
}

#[tokio::test]
async fn aggregate_without_compatible_sources_returns_empty_feed_without_dispatch() {
    let h = Harness::new().await;
    // Torznab accepts every identifier, so take it out of the aggregate.
    h.disable(&h.ids[2]).await;
    for query in [
        "t=movie&doubanid=42",
        "t=MOVIE&doubanid=42&offset=3&limit=2",
        "t=book&author=fixture&tvmazeid=42",
    ] {
        let (status, body) = h.get("all", query).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(titles(&body).is_empty());
        let expected_offset = if query.contains("offset=3") { "3" } else { "0" };
        assert_eq!(total(&body), (expected_offset.into(), "0".into()));
        assert!(h.take_hits().is_empty(), "{query}");
    }
}

#[tokio::test]
async fn aggregate_tv_search_merges_all_compatible_sources_and_paginates() {
    let h = Harness::new().await;
    let (status, body) = h.get("all", "t=tvsearch&q=fixture").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        titles(&body),
        ["Fixture Anime", "Fixture Feed", "Fixture Torznab"]
    );
    assert_eq!(h.contacted(), ["acgrip", "feed", "torznab", "unit3d"]);
    let (status, body) = h.get("all", "t=tvsearch&q=fixture&offset=1&limit=1").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(titles(&body), ["Fixture Feed"]);
    // Existing behavior: the merged fetch is capped at offset + limit.
    assert_eq!(total(&body), ("1".into(), "2".into()));
}

#[tokio::test]
async fn global_errors_and_indexers_stay_unchanged_and_caps_advertise_usable_union() {
    let h = Harness::new().await;
    let (status, body) = h
        .raw("/torznab/all/api?apikey=wrong&t=movie&doubanid=42")
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(error_code(&body), "100");
    for (query, code) in [
        ("t=bogus", "203"),
        ("t=", "203"),
        ("t=movie&offset=oops", "201"),
        ("t=movie&imdbid=bad", "201"),
        ("t=movie&cat=x", "201"),
        ("t=movie&offset=100001", "201"),
    ] {
        let (status, body) = h.get("all", query).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
        assert_eq!(error_code(&body), code, "{query}");
    }
    let (status, body) = h.get("missing", "t=movie").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error_code(&body), "201");
    assert!(h.take_hits().is_empty());

    // Aggregate caps advertise modes and ID parameters supported by any source.
    let (status, body) = h.get("all", "t=caps").await;
    assert_eq!(status, StatusCode::OK);
    let doc = roxmltree::Document::parse(&body).unwrap();
    let mode = |name: &str| {
        doc.descendants()
            .find(|n| n.tag_name().name() == name)
            .unwrap()
            .attribute("available")
            .unwrap()
            .to_owned()
    };
    assert_eq!(mode("movie-search"), "yes");
    assert_eq!(mode("tv-search"), "yes");
    for name in ["movie-search", "tv-search"] {
        let params = doc
            .descendants()
            .find(|n| n.tag_name().name() == name)
            .unwrap()
            .attribute("supportedParams")
            .unwrap();
        for id in ["imdbid", "tmdbid", "tvdbid"] {
            assert!(params.split(',').any(|p| p == id), "{name}: {params}");
        }
        assert!(!params.split(',').any(|p| p == "tvmazeid"));
    }
    // The same TV-only source remains explicit about its own limitations.
    let (status, single) = h.get(&h.ids[0], "t=caps").await;
    assert_eq!(status, StatusCode::OK);
    let single = roxmltree::Document::parse(&single).unwrap();
    assert_eq!(
        single
            .descendants()
            .find(|n| n.tag_name().name() == "movie-search")
            .unwrap()
            .attribute("available"),
        Some("no")
    );
    let categories: Vec<_> = doc
        .descendants()
        .filter(|n| n.tag_name().name() == "category")
        .map(|n| n.attribute("id").unwrap().to_owned())
        .collect();
    assert_eq!(categories, ["2000", "5000"]);
    let (status, body) = h.get("all", "t=indexers").await;
    assert_eq!(status, StatusCode::OK);
    let doc = roxmltree::Document::parse(&body).unwrap();
    assert_eq!(
        doc.descendants()
            .filter(|n| n.tag_name().name() == "indexer")
            .count(),
        4
    );
    assert!(h.take_hits().is_empty());
}
