use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    routing::get,
    Router,
};
use rackett::{engine::Engine, models::Indexer, AppState};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tower::ServiceExt;
const TORRENT: &[u8] = b"d4:infod4:name6:ubuntu6:lengthi42eee";
const SECRET: &str = "fixture-private-source-credential";
struct Harness {
    app: Router,
    engine: Arc<Engine>,
    id: String,
    key: String,
    count: Arc<AtomicUsize>,
    _dir: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Harness {
    async fn new() -> Self {
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let unit3d_count = count.clone();
        let upstream=Router::new().route("/feed",get(||async{"<rss version=\"2.0\"><channel><title>Fixture</title><item><title>A &amp; B &lt;Linux&gt;</title><guid>urn:fixture:1</guid><enclosure url=\"/torrent\" length=\"42\" type=\"application/x-bittorrent\"/></item></channel></rss>"})).route("/secret-feed",get(||async{"<rss version=\"2.0\" xmlns:torznab=\"http://torznab.com/schemas/2015/feed\"><channel><item><title>Secret-bearing source</title><guid>https://example.com/private?passkey=fixture-private-source-credential</guid><link>https://example.com/details?token=fixture-private-source-credential</link><enclosure url=\"/torrent?passkey=fixture-private-source-credential\" length=\"42\" type=\"application/x-bittorrent\"/><torznab:attr name=\"coverurl\" value=\"https://example.com/cover?apikey=fixture-private-source-credential\"/><torznab:attr name=\"magneturl\" value=\"magnet:?xt=urn:btih:123&amp;passkey=fixture-private-source-credential\"/><torznab:attr name=\"custom_auth\" value=\"fixture-private-source-credential\"/></item></channel></rss>"})).route("/api/torrents/filter",get(move||{let count=unit3d_count.clone();async move{count.fetch_add(1,Ordering::SeqCst);(StatusCode::INTERNAL_SERVER_ERROR,"Unexpected UNIT3D request")}})).route("/bad-feed",get(||async{"<rss version=\"2.0\"><channel><item><title>Bad download</title><guid>urn:fixture:bad</guid><enclosure url=\"/html\" type=\"application/x-bittorrent\"/></item></channel></rss>"})).route("/torrent",get(move||{let c=c.clone();async move{c.fetch_add(1,Ordering::SeqCst);TORRENT}})).route("/html",get(||async{"<html>Login required; fixture-private-source-credential</html>"})).route("/failure",get(||async{(StatusCode::INTERNAL_SERVER_ERROR,"fixture-private-source-credential server stack trace")}));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let engine = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
        let i = engine
            .put_indexer(
                Indexer {
                    name: "Fixture source".into(),
                    url: format!("http://{addr}/feed"),
                    api_key: SECRET.into(),
                    cookie: format!("session={SECRET}"),
                    request_delay_ms: 0,
                    categories: vec![2000],
                    ..Default::default()
                },
                false,
            )
            .await
            .unwrap();
        let key = engine.settings().await.api_key;
        let app = Router::new()
            .merge(rackett::api::router())
            .merge(rackett::protocol::router())
            .with_state(AppState {
                engine: engine.clone(),
            });
        Self {
            app,
            engine,
            id: i.id,
            key,
            count,
            _dir: dir,
            server,
        }
    }
    async fn get(&self, path: &str) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        self.request("GET", path, None).await
    }
    async fn request(
        &self,
        method: &str,
        path: &str,
        origin: Option<&str>,
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "127.0.0.1:9117");
        if let Some(origin) = origin {
            req = req.header("origin", origin);
        }
        let response = self
            .app
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec();
        (status, headers, body)
    }
    fn path(&self, suffix: &str) -> String {
        format!(
            "/api/v2.0/indexers/{}/results/torznab/api?apikey={}&{suffix}",
            self.id, self.key
        )
    }
}
fn as_text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}
fn download_path(xml: &str) -> String {
    let doc = roxmltree::Document::parse(xml).unwrap();
    let link = doc
        .descendants()
        .find(|n| n.tag_name().name() == "enclosure")
        .unwrap()
        .attribute("url")
        .unwrap();
    assert!(link.starts_with("http://127.0.0.1:9117/dl/"));
    let u = url::Url::parse(link).unwrap();
    format!("{}?{}", u.path(), u.query().unwrap())
}

#[tokio::test]
async fn caps_auth_errors_and_parameter_validation_are_xml() {
    let h = Harness::new().await;
    let (status, headers, body) = h.get(&h.path("t=caps")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers["content-type"].to_str().unwrap().contains("xml"));
    let doc = roxmltree::Document::parse(as_text(&body)).unwrap();
    assert_eq!(doc.root_element().tag_name().name(), "caps");
    assert!(doc
        .descendants()
        .any(|n| n.tag_name().name() == "category" && n.attribute("id") == Some("2000")));
    for (path, expected) in [
        (h.path("t=search&limit=wrong"), StatusCode::BAD_REQUEST),
        (h.path("t=search&cat=2000,invalid"), StatusCode::BAD_REQUEST),
        (
            h.path("t=caps").replace(&h.key, "wrong"),
            StatusCode::UNAUTHORIZED,
        ),
        (
            h.path("t=caps").replace(&h.id, "unknown"),
            StatusCode::NOT_FOUND,
        ),
    ] {
        let (status, _, body) = h.get(&path).await;
        assert_eq!(status, expected);
        assert_eq!(
            roxmltree::Document::parse(as_text(&body))
                .unwrap()
                .root_element()
                .tag_name()
                .name(),
            "error"
        );
        assert!(!as_text(&body).contains(SECRET));
    }
}

#[tokio::test]
async fn search_escapes_xml_and_download_preserves_torrent_bytes() {
    let h = Harness::new().await;
    let (status, _, body) = h.get(&h.path("t=search&q=Linux")).await;
    assert_eq!(status, StatusCode::OK);
    let text = as_text(&body);
    assert!(text.contains("A &amp; B &lt;Linux&gt;"));
    assert!(!text.contains(SECRET));
    let path = download_path(text);
    assert_eq!(h.count.load(Ordering::SeqCst), 0);
    let (status, headers, bytes) = h.get(&path).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "application/x-bittorrent");
    assert_eq!(bytes, TORRENT);
    assert_eq!(h.count.load(Ordering::SeqCst), 1);
    let (status, _, _) = h.get(&path.replace(&h.key, "wrong")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn invalid_torrent_and_upstream_errors_do_not_expose_credentials() {
    let h = Harness::new().await;
    let mut i = h.engine.indexer(&h.id).await.unwrap();
    i.url = i.url.replace("/feed", "/bad-feed");
    h.engine.put_indexer(i.clone(), true).await.unwrap();
    let (_, _, body) = h.get(&h.path("t=search")).await;
    let (status, _, body) = h.get(&download_path(as_text(&body))).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(!as_text(&body).contains(SECRET));
    assert!(!as_text(&body).contains("<html>"));
    i.url = i.url.replace("/bad-feed", "/failure");
    h.engine.put_indexer(i, true).await.unwrap();
    let (status, _, body) = h.get(&h.path("t=search")).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(!as_text(&body).contains(SECRET));
    assert!(!as_text(&body).contains("stack trace"));
}

#[tokio::test]
async fn potato_then_torznab_keeps_cached_source_link_unmodified() {
    let h = Harness::new().await;
    let potato = format!(
        "/api/v2.0/indexers/{}/results/potato/api?passkey={}&search=Linux",
        h.id, h.key
    );
    let (status, _, body) = h.get(&potato).await;
    assert_eq!(status, StatusCode::OK);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let url = json["results"][0]["download_url"].as_str().unwrap();
    assert!(url.starts_with("http://127.0.0.1:9117/dl/"));
    assert!(!as_text(&body).contains(SECRET));
    let first = h
        .engine
        .search(
            vec![h.id.clone()],
            rackett::models::SearchQuery {
                params: [("t".into(), "movie".into()), ("q".into(), "Linux".into())].into(),
            },
        )
        .await;
    assert!(first.cached > 0);
    assert!(first.results[0].link.ends_with("/torrent"));
    let (status, _, body) = h.get(&h.path("t=movie&q=Linux")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, bytes) = h.get(&download_path(as_text(&body))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, TORRENT);
    assert_eq!(h.count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn management_redacts_secrets_and_rotation_invalidates_old_protocol_key() {
    let h = Harness::new().await;
    let (status, _, body) = h.get("/api/indexers").await;
    assert_eq!(status, StatusCode::OK);
    let list: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(list[0].get("api_key").is_none());
    assert!(list[0].get("cookie").is_none());
    assert!(list[0].get("definition").is_none());
    assert!(!as_text(&body).contains(SECRET));
    let (status, _, body) = h
        .request(
            "POST",
            "/api/settings/rotate-key",
            Some("http://127.0.0.1:9117"),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let settings: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let new = settings["api_key"].as_str().unwrap();
    assert_ne!(new, h.key);
    assert_eq!(h.get(&h.path("t=caps")).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        h.get(&h.path("t=caps").replace(&h.key, new)).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn authenticated_source_urls_and_metadata_are_never_published() {
    let h = Harness::new().await;
    let mut indexer = h.engine.indexer(&h.id).await.unwrap();
    indexer.url = indexer.url.replace(
        "/feed",
        &format!("/secret-feed?apikey={SECRET}&rsskey={SECRET}"),
    );
    h.engine.put_indexer(indexer, true).await.unwrap();
    let (status, _, body) = h.get("/api/indexers").await;
    assert_eq!(status, StatusCode::OK);
    let indexers: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(indexers[0]["url_redacted"], true);
    let url = url::Url::parse(indexers[0]["url"].as_str().unwrap()).unwrap();
    assert!(url.query_pairs().all(|(_, value)| value == "REDACTED"));
    assert!(!as_text(&body).contains(SECRET));

    let (status, _, body) = h.get(&h.path("t=movie&q=Secret")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!as_text(&body).contains(SECRET));
    let xml = roxmltree::Document::parse(as_text(&body)).unwrap();
    let guid = xml
        .descendants()
        .find(|n| n.tag_name().name() == "guid")
        .unwrap()
        .text()
        .unwrap()
        .to_owned();
    assert!(guid.starts_with("urn:rackett:"));
    assert!(!xml.descendants().any(|n| n.tag_name().name() == "comments"));
    assert!(!xml.descendants().any(|n| matches!(
        n.attribute("name"),
        Some("coverurl" | "magneturl" | "custom_auth")
    )));
    let download = download_path(as_text(&body));
    let (status, _, torrent) = h.get(&download).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(torrent, TORRENT);

    let potato = format!(
        "/api/v2.0/indexers/{}/results/potato/api?apikey={}&search=Secret",
        h.id, h.key
    );
    let (status, _, body) = h.get(&potato).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!as_text(&body).contains(SECRET));
    let potato: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(potato["results"][0]["torrent_id"], guid);
    assert!(potato["results"][0]["details_url"].is_null());
    assert!(potato["results"][0]["download_url"]
        .as_str()
        .unwrap()
        .starts_with("http://127.0.0.1:9117/dl/"));

    let (status, _, body) = h.get("/api/search?q=Secret").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!as_text(&body).contains(SECRET));
    let results: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(results["results"][0]["guid"], guid);
    assert!(results["results"][0]["details"].is_null());
    assert!(results["results"][0]["link"]
        .as_str()
        .unwrap()
        .starts_with("http://127.0.0.1:9117/dl/"));
    assert!(results["results"][0]["attributes"]
        .as_object()
        .unwrap()
        .is_empty());
    // Public presentation must leave the authenticated source URL intact internally.
    let raw = h
        .engine
        .search(vec![h.id.clone()], rackett::models::SearchQuery::default())
        .await;
    assert!(raw.results[0].link.contains(SECRET));
    assert!(raw.results[0].guid.contains(SECRET));
}

#[tokio::test]
async fn unit3d_unsupported_identifier_is_rejected_before_network_access() {
    let h = Harness::new().await;
    let mut source = h.engine.indexer(&h.id).await.unwrap();
    source.kind = "unit3d".into();
    source.url = source.url.trim_end_matches("feed").to_owned();
    h.engine.put_indexer(source, true).await.unwrap();
    let (status, _, body) = h.get(&h.path("t=tvsearch&tvmazeid=12345")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let xml = roxmltree::Document::parse(as_text(&body)).unwrap();
    assert_eq!(xml.root_element().tag_name().name(), "error");
    assert_eq!(xml.root_element().attribute("code"), Some("203"));
    assert!(xml
        .root_element()
        .attribute("description")
        .unwrap()
        .contains("tvmazeid"));
    assert_eq!(h.count.load(Ordering::SeqCst), 0);
}
