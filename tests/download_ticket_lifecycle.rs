//! Download links are permanent (docs/permanent-download-links.md): they survive restarts
//! and edits that keep a source's URL and credentials, and retire when those change, when
//! the source is deleted, or when the producing runtime was replaced before presentation.
use axum::{
    body::{to_bytes, Body},
    extract::Query,
    http::{HeaderMap, Request, StatusCode},
    routing::get,
    Json, Router,
};
use rackett::{
    engine::{DownloadExpired, Engine},
    models::{Indexer, Release, SearchQuery, SearchResponse},
    AppState,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;
use tower::ServiceExt;

const TORRENT: &[u8] = b"d4:infod4:name6:ubuntu6:lengthi42eee";
const MAGNET: &str = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=fixture";
const BASE: &str = "http://127.0.0.1:9117";

/// `entered` and `release` signals for one held upstream search.
type Hold = (Arc<Notify>, Arc<Notify>);

/// Loopback upstream that records every credential it receives.
#[derive(Clone, Default)]
struct Upstream {
    /// Milkie search auth headers, in arrival order.
    searches: Arc<Mutex<Vec<String>>>,
    /// Credentials carried by every download request, in arrival order.
    downloads: Arc<Mutex<Vec<String>>>,
    /// When armed, the next Milkie search signals `entered` and waits for `release`.
    hold: Arc<Mutex<Option<Hold>>>,
}
impl Upstream {
    fn arm(&self) -> Hold {
        let pair = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        *self.hold.lock().unwrap() = Some(pair.clone());
        pair
    }
    fn downloads(&self) -> Vec<String> {
        self.downloads.lock().unwrap().clone()
    }
    fn searches(&self) -> Vec<String> {
        self.searches.lock().unwrap().clone()
    }
    async fn serve(&self) -> (String, tokio::task::JoinHandle<()>) {
        let (searches, hold) = (self.searches.clone(), self.hold.clone());
        let (milkie_downloads, rss_downloads) = (self.downloads.clone(), self.downloads.clone());
        let router = Router::new()
            .route(
                "/api/v1/torrents",
                get(move |headers: HeaderMap| {
                    let (searches, hold) = (searches.clone(), hold.clone());
                    async move {
                        searches.lock().unwrap().push(
                            headers
                                .get("x-milkie-auth")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("")
                                .to_owned(),
                        );
                        let held = hold.lock().unwrap().take();
                        if let Some((entered, release)) = held {
                            entered.notify_one();
                            release.notified().await;
                        }
                        Json(json!({"hits":1,"torrents":[{"id":"42","category":1,
                            "releaseName":"Lifecycle fixture","createdAt":"2026-10-27T00:00:00Z",
                            "size":123,"downloaded":1,"seeders":1,"leechers":0}]}))
                    }
                }),
            )
            .route(
                "/api/v1/torrents/42/torrent",
                get(move |Query(q): Query<BTreeMap<String, String>>| {
                    let downloads = milkie_downloads.clone();
                    async move {
                        downloads
                            .lock()
                            .unwrap()
                            .push(q.get("key").cloned().unwrap_or_default());
                        TORRENT
                    }
                }),
            )
            .route(
                "/feed",
                get(|| async {
                    format!("<rss version=\"2.0\"><channel><title>Fixture</title>\
                        <item><title>Feed release</title><guid>urn:fixture:1</guid>\
                        <enclosure url=\"/torrent?passkey=feed-passkey\" length=\"42\" type=\"application/x-bittorrent\"/></item>\
                        <item><title>Magnet release</title><guid>urn:fixture:2</guid>\
                        <link>{}</link></item></channel></rss>", MAGNET.replace('&', "&amp;"))
                }),
            )
            .route(
                "/torrent",
                get(move |Query(q): Query<BTreeMap<String, String>>| {
                    let downloads = rss_downloads.clone();
                    async move {
                        downloads
                            .lock()
                            .unwrap()
                            .push(q.get("passkey").cloned().unwrap_or_default());
                        TORRENT
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        (
            url,
            tokio::spawn(async move { axum::serve(listener, router).await.unwrap() }),
        )
    }
}

fn milkie(url: &str, key: &str) -> Indexer {
    Indexer {
        name: "Fixture Milkie".into(),
        kind: "cardigann".into(),
        url: url.into(),
        request_delay_ms: 0,
        definition: Some(json!({"builtin_id":"milkie","settings":{"apikey":key}})),
        ..Default::default()
    }
}
fn feed(url: &str) -> Indexer {
    Indexer {
        name: "Fixture feed".into(),
        url: format!("{url}feed"),
        request_delay_ms: 0,
        ..Default::default()
    }
}
async fn rekey(engine: &Arc<Engine>, id: &str, key: &str) -> Indexer {
    let mut edited = engine.indexer(id).await.unwrap();
    edited.definition.as_mut().unwrap()["settings"]["apikey"] = json!(key);
    engine.put_indexer(edited, true).await.unwrap()
}
fn app(engine: &Arc<Engine>) -> Router {
    Router::new()
        .merge(rackett::api::router())
        .merge(rackett::protocol::router())
        .with_state(AppState {
            engine: engine.clone(),
        })
}
async fn get_path(app: &Router, path: &str) -> (StatusCode, Vec<u8>) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header("host", "127.0.0.1:9117")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, body.to_vec())
}
/// Fetch a presented `/dl/` link through the protocol router.
async fn fetch(app: &Router, link: &str) -> (StatusCode, Vec<u8>) {
    assert!(link.starts_with(&format!("{BASE}/dl/")), "{link}");
    let u = url::Url::parse(link).unwrap();
    get_path(app, &format!("{}?{}", u.path(), u.query().unwrap())).await
}
fn assert_expired((status, body): (StatusCode, Vec<u8>)) {
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(String::from_utf8(body).unwrap().contains(
        "Download link retired: the source was removed, disabled or changed; search again"
    ));
}
fn assert_torrent((status, body): (StatusCode, Vec<u8>)) {
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, TORRENT);
}
async fn present(engine: &std::sync::Arc<Engine>, mut out: SearchResponse) -> Vec<String> {
    let key = engine.settings().await.api_key;
    let links = engine.links(&out.results, &key, BASE).await;
    rackett::protocol::present_results(&mut out, &links);
    out.results.into_iter().map(|r| r.link).collect()
}
async fn search_links(engine: &std::sync::Arc<Engine>, id: &str) -> Vec<String> {
    let out = engine
        .search(vec![id.to_owned()], SearchQuery::default())
        .await;
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    present(engine, out).await
}
fn uncached() -> SearchQuery {
    SearchQuery {
        params: BTreeMap::from([("cache".into(), "false".into())]),
    }
}

#[tokio::test]
async fn milkie_key_rotation_retires_old_ticket_before_any_upstream_request() {
    let upstream = Upstream::default();
    let (url, server) = upstream.serve().await;
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
    let app = app(&engine);
    let source = engine
        .put_indexer(milkie(&url, "old-key"), false)
        .await
        .unwrap();
    let old = search_links(&engine, &source.id).await;
    assert!(!old[0].contains("old-key"));

    rekey(&engine, &source.id, "new-key").await;
    assert_expired(fetch(&app, &old[0]).await);
    assert!(upstream.downloads().is_empty());

    // The rotated runtime has its own cache, so the next search reaches upstream anew.
    let new = search_links(&engine, &source.id).await;
    assert_eq!(upstream.searches(), ["old-key", "new-key"]);
    assert_torrent(fetch(&app, &new[0]).await);
    assert_eq!(upstream.downloads(), ["new-key"]);
    assert_expired(fetch(&app, &old[0]).await);
    assert_eq!(upstream.downloads(), ["new-key"]);
    server.abort();
}

#[tokio::test]
async fn links_survive_renames_and_restarts_and_retire_on_delete() {
    let upstream = Upstream::default();
    let (url, server) = upstream.serve().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let engine = Arc::new(Engine::new(path.clone()).await.unwrap());
    let app = app(&engine);
    let source = engine.put_indexer(feed(&url), false).await.unwrap();

    // A name-only edit replaces the runtime but keeps URL and credentials: links stay.
    let first = search_links(&engine, &source.id).await;
    let mut renamed = source.clone();
    renamed.name = "Renamed feed".into();
    engine.put_indexer(renamed, true).await.unwrap();
    assert_torrent(fetch(&app, &first[0]).await);
    assert_eq!(upstream.downloads(), ["feed-passkey"]);

    // Cached releases keep the identity of the runtime that cached them.
    let fresh = engine
        .search(vec![source.id.clone()], SearchQuery::default())
        .await;
    assert_eq!(fresh.cached, 0);
    let fresh = present(&engine, fresh).await;
    let cached = engine
        .search(vec![source.id.clone()], SearchQuery::default())
        .await;
    assert_eq!(cached.cached, 1);
    assert_eq!(cached.results[0].indexer, "Renamed feed");
    let cached = present(&engine, cached).await;
    assert_torrent(fetch(&app, &fresh[0]).await);
    assert_torrent(fetch(&app, &cached[0]).await);
    assert_eq!(upstream.downloads().len(), 3);

    engine.delete_indexer(&source.id).await.unwrap();
    assert_expired(fetch(&app, &cached[0]).await);

    // A restarted engine reads the same link key: links issued before it still work.
    let survivor = engine.put_indexer(feed(&url), false).await.unwrap();
    let before_restart = search_links(&engine, &survivor.id).await;
    drop(app);
    drop(engine);
    let engine = Arc::new(Engine::new(path).await.unwrap());
    let app = self::app(&engine);
    assert!(engine.indexer(&survivor.id).await.is_some());
    assert_torrent(fetch(&app, &before_restart[0]).await);
    assert_eq!(upstream.downloads().len(), 4);
    server.abort();
}

#[tokio::test]
async fn search_racing_edit_keeps_old_identity_and_never_mixes_metadata() {
    let upstream = Upstream::default();
    let (url, server) = upstream.serve().await;
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
    let app = app(&engine);
    let mut source = milkie(&url, "old-key");
    source.name = "Old name".into();
    let source = engine.put_indexer(source, false).await.unwrap();

    let (entered, release) = upstream.arm();
    let racing = tokio::spawn({
        let (engine, id) = (engine.clone(), source.id.clone());
        async move { engine.search(vec![id], SearchQuery::default()).await }
    });
    entered.notified().await;
    // The edit fully completes while the old-runtime search is still upstream.
    let mut edited = engine.indexer(&source.id).await.unwrap();
    edited.name = "New name".into();
    edited.definition.as_mut().unwrap()["settings"]["apikey"] = json!("new-key");
    engine.put_indexer(edited, true).await.unwrap();
    release.notify_one();
    let raced = racing.await.unwrap();
    assert!(raced.errors.is_empty(), "{:?}", raced.errors);
    // Results describe the old runtime consistently: old name and old credential.
    assert_eq!(raced.results[0].indexer, "Old name");
    assert!(raced.results[0].link.contains("key=old-key"));
    // Presentation after the edit still captures the old identity.
    let raced = present(&engine, raced).await;
    assert_expired(fetch(&app, &raced[0]).await);
    assert!(upstream.downloads().is_empty());

    let current = engine
        .search(vec![source.id.clone()], SearchQuery::default())
        .await;
    assert_eq!(current.results[0].indexer, "New name");
    assert_eq!(current.cached, 0);
    let current = present(&engine, current).await;
    assert_torrent(fetch(&app, &current[0]).await);
    assert_eq!(upstream.searches(), ["old-key", "new-key"]);
    assert_eq!(upstream.downloads(), ["new-key"]);
    server.abort();
}

#[tokio::test]
async fn download_queued_behind_rate_gate_revalidates_before_sending() {
    let upstream = Upstream::default();
    let (url, server) = upstream.serve().await;
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
    let source = engine
        .put_indexer(milkie(&url, "old-key"), false)
        .await
        .unwrap();
    let out = engine
        .search(vec![source.id.clone()], SearchQuery::default())
        .await;
    let release = out.results[0].clone();

    // Hold the runtime's rate gate with an in-flight search, then queue a download.
    let gated = upstream.clone();
    let hold_gate = |engine: Arc<Engine>, id: String| {
        let (entered, release) = gated.arm();
        async move {
            let holder = tokio::spawn(async move { engine.search(vec![id], uncached()).await });
            entered.notified().await;
            (holder, release)
        }
    };

    // Control: without an edit the queued download proceeds once the gate opens.
    let (holder, open) = hold_gate(engine.clone(), source.id.clone()).await;
    let mut queued = Box::pin(engine.download(&source.id, release.source_epoch, &release.link));
    assert!(futures_util::poll!(queued.as_mut()).is_pending());
    open.notify_one();
    holder.await.unwrap();
    assert!(matches!(
        queued.await.unwrap(),
        rackett::engine::Download::Torrent { bytes, .. } if bytes == TORRENT
    ));
    assert_eq!(upstream.downloads(), ["old-key"]);

    // The ticket passed its first check, then the source was edited while it waited.
    let (holder, open) = hold_gate(engine.clone(), source.id.clone()).await;
    let mut queued = Box::pin(engine.download(&source.id, release.source_epoch, &release.link));
    assert!(futures_util::poll!(queued.as_mut()).is_pending());
    rekey(&engine, &source.id, "new-key").await;
    open.notify_one();
    holder.await.unwrap();
    assert!(queued.await.unwrap_err().is::<DownloadExpired>());
    assert_eq!(upstream.downloads(), ["old-key"]);
    server.abort();
}

#[tokio::test]
async fn torznab_and_potato_links_follow_source_configuration_and_magnets_pass_through() {
    let upstream = Upstream::default();
    let (url, server) = upstream.serve().await;
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
    let app = app(&engine);
    let source = engine.put_indexer(feed(&url), false).await.unwrap();
    let key = engine.settings().await.api_key;
    let torznab = |app: Router| {
        let path = format!("/torznab/{}/api?t=search&apikey={key}", source.id);
        async move {
            let (status, body) = get_path(&app, &path).await;
            assert_eq!(status, StatusCode::OK);
            let text = String::from_utf8(body).unwrap();
            let doc = roxmltree::Document::parse(&text).unwrap();
            doc.descendants()
                .filter(|n| n.tag_name().name() == "enclosure")
                .map(|n| n.attribute("url").unwrap().to_owned())
                .collect::<Vec<_>>()
        }
    };
    let potato = |app: Router| {
        let path = format!("/potato/{}?apikey={key}", source.id);
        async move {
            let (status, body) = get_path(&app, &path).await;
            assert_eq!(status, StatusCode::OK);
            let v: Value = serde_json::from_slice(&body).unwrap();
            v["results"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["download_url"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        }
    };
    let split = |links: Vec<String>| {
        assert_eq!(links.len(), 2);
        assert!(links.iter().any(|l| l == MAGNET), "{links:?}");
        links.into_iter().find(|l| l != MAGNET).unwrap()
    };
    let old_torznab = split(torznab(app.clone()).await);
    let old_potato = split(potato(app.clone()).await);
    assert_torrent(fetch(&app, &old_torznab).await);
    assert_torrent(fetch(&app, &old_potato).await);
    assert_eq!(upstream.downloads().len(), 2);

    // A delay-only edit keeps both links.
    let mut edited = source.clone();
    edited.request_delay_ms = 1;
    let edited = engine.put_indexer(edited, true).await.unwrap();
    assert_torrent(fetch(&app, &old_torznab).await);
    assert_torrent(fetch(&app, &old_potato).await);
    assert_eq!(upstream.downloads().len(), 4);

    // Changing the source URL, even on the same origin, retires them before any request.
    let mut moved = edited.clone();
    moved.url = format!("{url}feed?edition=2");
    engine.put_indexer(moved, true).await.unwrap();
    assert_expired(fetch(&app, &old_torznab).await);
    assert_expired(fetch(&app, &old_potato).await);
    assert_eq!(upstream.downloads().len(), 4);

    // New searches mint working links; magnets remain untouched passthrough links.
    assert_torrent(fetch(&app, &split(torznab(app.clone()).await)).await);
    assert_torrent(fetch(&app, &split(potato(app.clone()).await)).await);
    assert_eq!(upstream.downloads().len(), 6);
    server.abort();
}

#[tokio::test]
async fn epoch_is_never_serialized_and_unstamped_tickets_are_rejected() {
    let stamped = Release {
        title: "Fixture".into(),
        source_epoch: 9,
        ..Default::default()
    };
    let value = serde_json::to_value(&stamped).unwrap();
    assert!(value.get("source_epoch").is_none());
    let mut forged = value.clone();
    forged["source_epoch"] = json!(9);
    let parsed: Release = serde_json::from_value(forged).unwrap();
    assert_eq!(parsed.source_epoch, 0);

    let upstream = Upstream::default();
    let (url, server) = upstream.serve().await;
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
    let app = app(&engine);
    let source = engine.put_indexer(feed(&url), false).await.unwrap();
    let key = engine.settings().await.api_key;

    let (status, body) = get_path(&app, &format!("/api/search?indexers={}", source.id)).await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(body).unwrap();
    assert!(!text.contains("source_epoch"));
    let api: Value = serde_json::from_str(&text).unwrap();
    let link = api["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["link"].as_str().unwrap())
        .find(|l| *l != MAGNET)
        .unwrap();
    let u = url::Url::parse(link).unwrap();
    assert_torrent(get_path(&app, &format!("{}?{}", u.path(), u.query().unwrap())).await);

    // A release that never passed through the engine carries epoch zero.
    let mut unstamped = SearchResponse {
        results: vec![Release {
            indexer_id: source.id.clone(),
            link: format!("{url}torrent?passkey=feed-passkey"),
            ..Default::default()
        }],
        ..Default::default()
    };
    let links = engine.links(&unstamped.results, &key, BASE).await;
    rackett::protocol::present_results(&mut unstamped, &links);
    assert_expired(fetch(&app, &unstamped.results[0].link).await);
    assert!(engine
        .download(&source.id, 0, &format!("{url}torrent"))
        .await
        .unwrap_err()
        .is::<DownloadExpired>());
    assert_eq!(upstream.downloads(), ["feed-passkey"]);
    server.abort();
}
