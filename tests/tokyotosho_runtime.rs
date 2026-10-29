//! Pinned Tokyo Toshokan selectors, synthetic loopback pages, no live requests.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    routing::get,
    Router,
};
use rackett::{
    builtin,
    builtin_runtime::{self, flaresolverr::Solver, Source},
    engine::Engine,
    models::{Indexer, SearchQuery},
    AppState,
};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tower::ServiceExt;

const MAGNET: &str = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567";
fn pair(magnet: Option<&str>, download: &str) -> String {
    format!(
        r#"<tr class="category_0"><td><a href="?cat=1">Anime</a></td>
<td class="desc-top"><a type="application/x-bittorrent" href="{download}">Fixture Anime S01E02</a>{}</td>
<td><a href="details.php?id=42">Details</a></td></tr>
<tr class="category_0"><td class="desc-bot">Submitter: x | Size: 512MB | Date: 2026-10-28 12:34 UTC | Comment: x</td>
<td class="stats"><span>12</span><span>3</span><span>9</span></td></tr>"#,
        magnet
            .map(|m| format!("<a href=\"{m}\">magnet</a>"))
            .unwrap_or_default()
    )
}
fn page(rows: &str) -> String {
    format!("<html><body><table class=\"listing\">{rows}</table></body></html>")
}
fn stock(url: &str) -> Indexer {
    Indexer {
        name: "Tokyo fixture".into(),
        kind: "cardigann".into(),
        url: url.into(),
        request_delay_ms: 0,
        definition: Some(json!({"builtin_id":"tokyotosho","settings":{"cat":"1"}})),
        ..Default::default()
    }
}
fn query(q: &str) -> SearchQuery {
    SearchQuery {
        params: [("q".into(), q.into()), ("cache".into(), "false".into())]
            .into_iter()
            .collect(),
    }
}
struct Fixture {
    url: String,
    body: Arc<Mutex<String>>,
    status: Arc<Mutex<StatusCode>>,
    seen: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn fixture() -> Fixture {
    let body = Arc::new(Mutex::new(page(&pair(
        Some(MAGNET),
        "https://external.example/file.torrent",
    ))));
    let status = Arc::new(Mutex::new(StatusCode::OK));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (b, s, h) = (body.clone(), status.clone(), seen.clone());
    let app = Router::new().fallback(get(move |uri: axum::http::Uri| {
        let (b, s, h) = (b.clone(), s.clone(), h.clone());
        async move {
            h.lock().unwrap().push(uri.to_string());
            (*s.lock().unwrap(), b.lock().unwrap().clone())
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    Fixture {
        url,
        body,
        status,
        seen,
        task: tokio::spawn(async move { axum::serve(listener, app).await.unwrap() }),
    }
}
async fn api(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("host", "127.0.0.1")
                .header("origin", "http://127.0.0.1")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn create_restart_edit_search_caps_and_magnet_presentation() {
    let f = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let engine = Arc::new(Engine::new(path.clone()).await.unwrap());
    let app = rackett::api::router().with_state(AppState {
        engine: engine.clone(),
    });
    let (status, created) = api(&app, "POST", "/api/indexers", json!(stock(&f.url))).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let id = created["id"].as_str().unwrap().to_owned();
    drop(app);
    drop(engine);
    let engine = Arc::new(Engine::new(path).await.unwrap());
    assert!(engine.test(&id).await.is_ok());
    let catalog = builtin::catalog()
        .iter()
        .find(|e| e["id"] == "tokyotosho")
        .unwrap();
    assert_eq!(catalog["ready"], true);
    assert_eq!(catalog["caps"]["tv-search"], json!(["q", "season", "ep"]));
    assert!(catalog["caps"].get("movie-search").is_none());
    let found = engine
        .search(vec![id.clone()], query("Fixture Anime"))
        .await;
    assert!(found.errors.is_empty(), "{:?}", found.errors);
    assert_eq!(found.results.len(), 1);
    let r = &found.results[0];
    assert_eq!(r.link, MAGNET);
    assert_eq!(r.title, "Fixture Anime S01E02");
    assert_eq!(r.size, Some(512 * 1024 * 1024));
    assert_eq!(r.seeders, Some(12));
    assert_eq!(r.peers, Some(15));
    assert_eq!(r.published.as_deref(), Some("2026-10-28T12:34:00Z"));
    assert!(r.categories.contains(&5070));
    assert_eq!(r.attributes["downloadvolumefactor"], "0");
    assert!(engine
        .search(vec![id.clone()], query("Unrelated"))
        .await
        .results
        .is_empty());
    let app = rackett::api::router()
        .merge(rackett::protocol::router())
        .with_state(AppState {
            engine: engine.clone(),
        });
    let (status, edited) = api(
        &app,
        "PATCH",
        &format!("/api/indexers/{id}"),
        json!({"definition":{"builtin_id":"tokyotosho","settings":{"cat":"3"}}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    let (status, result) = api(
        &app,
        "GET",
        &format!("/api/search?indexers={id}&q=Fixture&cache=false"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(result["results"][0]["link"], MAGNET);
    let settings = engine.settings().await;
    let response=app.clone().oneshot(Request::builder().uri(format!("/api/v2.0/indexers/{id}/results/torznab/api?apikey={}&t=tvsearch&q=Fixture&season=1&ep=2",settings.api_key)).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let xml = String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(xml.contains(MAGNET));
    assert!(!xml.contains("/dl/"));
    let seen = f.seen.lock().unwrap().clone();
    assert!(seen.iter().any(|p| p == "/index.php?cat=1"), "{seen:?}");
    assert!(
        seen.iter()
            .any(|p| p == "/search.php?terms=Fixture+Anime&cat=1"),
        "{seen:?}"
    );
    assert!(
        seen.iter()
            .any(|p| p.contains("terms=Fixture+S01E02&cat=3")),
        "{seen:?}"
    );
    let rt = builtin::Runtime::compile(&stock(&f.url)).unwrap();
    assert!(rt
        .validate_query(&SearchQuery {
            params: [("t".into(), "movie".into())].into_iter().collect()
        })
        .is_err());
}
#[tokio::test]
async fn containers_rows_http_errors_and_download_boundaries_fail_closed() {
    let f = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(Engine::new(dir.path().join("c.json")).await.unwrap());
    let i = engine.put_indexer(stock(&f.url), false).await.unwrap();
    for body in [page(""), page("<tr><th>Title</th></tr>")] {
        *f.body.lock().unwrap() = body;
        let result = engine.search(vec![i.id.clone()], query("")).await;
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(result.results.is_empty());
    }
    for body in [
        "<table><tr><td>secret-response</td></tr></table>".into(),
        "<html>secret-response</html>".into(),
        page("<tr class=category_0><td>secret-response</td></tr>"),
        page(&pair(
            Some("magnet:?xt=urn:btih:bad#secret-response"),
            "https://external.example/file",
        )),
        page(&pair(None, "https://external.example/secret-response")),
        page(&pair(Some(MAGNET), "https://external.example/file"))
            .replace("<span>12</span>", "<span>bad</span>"),
    ] {
        *f.body.lock().unwrap() = body;
        let result = engine.search(vec![i.id.clone()], query("")).await;
        assert_eq!(result.errors.len(), 1);
        assert!(result.results.is_empty());
        assert!(!result.errors[0].message.contains("secret-response"));
    }
    *f.body.lock().unwrap() = page(&pair(None, "/download/42.torrent"));
    let result = engine.search(vec![i.id.clone()], query("")).await;
    assert!(result.errors.is_empty());
    assert_eq!(
        result.results[0].link,
        format!("{}download/42.torrent", f.url)
    );
    for status in [
        StatusCode::FORBIDDEN,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::BAD_GATEWAY,
    ] {
        *f.status.lock().unwrap() = status;
        *f.body.lock().unwrap() = "secret-response".into();
        let result = engine.search(vec![i.id.clone()], query("")).await;
        assert_eq!(result.errors.len(), 1);
        assert!(!result.errors[0].message.contains("secret-response"));
    }
    let mut bad = stock(&f.url);
    bad.definition = Some(json!({"builtin_id":"tokyotosho","settings":{"cat":"999"}}));
    assert!(builtin::Runtime::compile(&bad).is_err());
}
#[tokio::test]
async fn recognized_challenge_uses_existing_solver_and_keeps_magnets() {
    let f = fixture().await;
    *f.status.lock().unwrap() = StatusCode::FORBIDDEN;
    *f.body.lock().unwrap() =
        "<title>Just a moment...</title><script src='/cdn-cgi/challenge-platform/x'></script>"
            .into();
    let source_url = f.url.clone();
    let calls = Arc::new(Mutex::new(0));
    let seen = calls.clone();
    let solver=Router::new().route("/v1",axum::routing::post(move |axum::Json(v):axum::Json<Value>| {
        let url=source_url.clone(); let calls=seen.clone(); async move {
            *calls.lock().unwrap()+=1;
            assert_eq!(v["cmd"],"request.get"); assert_eq!(v["url"],format!("{url}search.php?terms=Fixture&cat=1"));
            axum::Json(json!({"status":"ok","solution":{"url":v["url"],"status":200,
                "response":page(&pair(Some(MAGNET),"https://external.example/a.torrent")),"cookies":[],"userAgent":"fixture-browser"}}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, solver).await.unwrap() });
    let i = stock(&f.url);
    let rt = builtin::Runtime::compile(&i).unwrap();
    let source = Source::new(&i, Some(Arc::new(Solver::new(&endpoint).unwrap()))).unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let rows = builtin_runtime::search_source(
        &rt,
        &client,
        &i,
        &query("Fixture"),
        &mut Instant::now(),
        &source,
        Instant::now() + Duration::from_secs(10),
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].link, MAGNET);
    assert_eq!(*calls.lock().unwrap(), 1);
    task.abort();
}
