//! Synthetic pages from unchanged Musopia selectors; no live tracker or credentials.
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{HeaderMap, Request, StatusCode, Uri},
    response::IntoResponse,
    routing::get,
    Router,
};
use rackett::{engine::Engine, models::Indexer, AppState};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

const COOKIE: &str = "session=fixture-private-cookie";
const ROTATED: &str = "session=fixture-rotated-cookie";
const AUTH: &str = "<a href=\"logout.php\">Logout</a>";
const TORRENT: &[u8] = b"d4:infod4:name6:ubuntu6:lengthi42eee";
const BASE: &str = "http://127.0.0.1:9117";
fn page() -> String {
    format!(
        "{AUTH}<table class=torrents><tbody><tr>\
        <td class=rowfollow><a href='?cat=407'>Audio</a></td>\
        <td class=rowfollow><a href='details.php?id=42'>Fixture Album</a>\
        <a href='download.php?id=42&amp;passkey=fixture-link-secret'>Download</a>\
        <img data-src='/poster.jpg'><img class=pro_free> Description</td>\
        <td class=rowfollow></td>\
        <td class=rowfollow><span title='2026-10-28 10:00:00'>age</span></td>\
        <td class=rowfollow>1 GB</td><td class=rowfollow>4</td>\
        <td class=rowfollow>2</td><td class=rowfollow>3</td>\
        </tr></tbody></table>"
    )
}
#[derive(Clone)]
struct Upstream {
    seen: Arc<Mutex<Vec<(String, String)>>>,
    /// A response override, restricted to one path. None serves authenticated fixtures.
    failure: Arc<Mutex<Option<(&'static str, &'static str)>>>,
}
impl Upstream {
    fn new() -> Self {
        Self {
            seen: Default::default(),
            failure: Default::default(),
        }
    }
    fn take(&self) -> Vec<(String, String)> {
        std::mem::take(&mut *self.seen.lock().unwrap())
    }
    fn fail(&self, path: &'static str, failure: &'static str) {
        *self.failure.lock().unwrap() = Some((path, failure));
    }
    async fn serve(&self) -> (String, tokio::task::JoinHandle<()>) {
        async fn respond(
            State(s): State<Upstream>,
            uri: Uri,
            h: HeaderMap,
        ) -> axum::response::Response {
            s.seen.lock().unwrap().push((
                uri.to_string(),
                h.get("cookie")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .into(),
            ));
            let failure = *s.failure.lock().unwrap();
            let failure = failure.filter(|(p, _)| *p == uri.path()).map(|(_, f)| f);
            let body = match failure {
                Some("expired") => "<form>fixture-private-cookie expired</form>".into(),
                Some("empty") => format!("{AUTH}<table class=torrents></table>"),
                Some("challenge-body") => format!("{AUTH}<script>window._cf_chl_opt={{}}</script>"),
                _ if uri.path() == "/download.php" => return TORRENT.into_response(),
                _ if uri.path() == "/torrents.php" => page(),
                _ => AUTH.into(),
            };
            let mut response =
                ([("content-type", "text/html; charset=utf-8")], body).into_response();
            match failure {
                Some("set-cookie") => {
                    response
                        .headers_mut()
                        .insert("set-cookie", "session=new".parse().unwrap());
                }
                Some("refresh") => {
                    response
                        .headers_mut()
                        .insert("refresh", "0;url=/login".parse().unwrap());
                }
                Some("challenge") => {
                    response
                        .headers_mut()
                        .insert("cf-mitigated", "challenge".parse().unwrap());
                }
                Some("redirect") => {
                    *response.status_mut() = StatusCode::FOUND;
                    response
                        .headers_mut()
                        .insert("location", "/login".parse().unwrap());
                }
                _ => {}
            }
            response
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let app = Router::new()
            .fallback(get(respond))
            .with_state(self.clone());
        (
            url,
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() }),
        )
    }
}
fn stock(url: &str) -> Indexer {
    Indexer {
        name: "Fixture Musopia".into(),
        kind: "cardigann".into(),
        url: url.into(),
        request_delay_ms: 0,
        definition: Some(
            json!({"builtin_id":"musopia", "settings":{"cookie":COOKIE,"freeleech":true}}),
        ),
        ..Default::default()
    }
}
fn app(e: &Arc<Engine>) -> Router {
    Router::new()
        .merge(rackett::api::router())
        .merge(rackett::protocol::router())
        .with_state(AppState { engine: e.clone() })
}
async fn request(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Vec<u8>) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("host", "127.0.0.1:9117")
                .header("origin", BASE)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    (
        response.status(),
        to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
}
async fn api(app: &Router, method: &str, path: &str, body: Value) -> Value {
    let (status, body) = request(app, method, path, body).await;
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let text = String::from_utf8(body).unwrap();
    for secret in [COOKIE, ROTATED, "fixture-link-secret"] {
        assert!(!text.contains(secret));
    }
    serde_json::from_str(&text).unwrap()
}
async fn search(app: &Router, id: &str) -> Value {
    api(
        app,
        "GET",
        &format!("/api/search?indexers={id}&q=Fixture&cat=3040"),
        Value::Null,
    )
    .await
}
async fn download(app: &Router, link: &str) -> (StatusCode, Vec<u8>) {
    let url = url::Url::parse(link).unwrap();
    assert!(url.path().starts_with("/dl/"));
    request(
        app,
        "GET",
        &format!("{}?{}", url.path(), url.query().unwrap()),
        Value::Null,
    )
    .await
}
fn link(result: &Value) -> &str {
    assert_eq!(result["errors"], json!([]), "{result}");
    assert_eq!(result["results"].as_array().unwrap().len(), 1, "{result}");
    result["results"][0]["link"].as_str().unwrap()
}
#[tokio::test]
async fn create_restart_search_test_ticket_and_credentials_use_one_runtime() {
    let upstream = Upstream::new();
    let (url, server) = upstream.serve().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let engine = Arc::new(Engine::new(path.clone()).await.unwrap());
    let created = api(&app(&engine), "POST", "/api/indexers", json!(stock(&url))).await;
    let id = created["id"].as_str().unwrap();
    assert!(upstream.take().is_empty()); // Saving validates configuration, never logs in.
    drop(engine);
    let engine = Arc::new(Engine::new(path).await.unwrap());
    let app = app(&engine);
    assert_eq!(
        engine.indexer(id).await.unwrap().definition.unwrap()["settings"]["cookie"],
        COOKIE
    );
    let public = api(&app, "GET", "/api/indexers", Value::Null).await;
    assert!(public[0]["definition"]["settings"].get("cookie").is_none());
    let out = search(&app, id).await;
    let ticket = link(&out);
    assert_eq!(out["results"][0]["title"], "Fixture Album");
    assert_eq!(out["results"][0]["size"], 1073741824_u64);
    assert_eq!(out["results"][0]["seeders"], 4);
    assert_eq!(out["results"][0]["peers"], 6);
    assert!(out["results"][0]["categories"]
        .as_array()
        .unwrap()
        .contains(&json!(3040)));
    let seen = upstream.take();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0], ("/index.php".into(), COOKIE.into()));
    let request = url::Url::parse(&format!("{BASE}{}", seen[1].0)).unwrap();
    let params: std::collections::BTreeMap<_, _> = request.query_pairs().collect();
    for (key, value) in [
        ("search", "Fixture"),
        ("cat407", "1"),
        ("cat408", "1"),
        ("spstate", "2"),
        ("sort", "4"),
        ("type", "desc"),
    ] {
        assert_eq!(params.get(key).map(|v| v.as_ref()), Some(value));
    }
    assert_eq!(seen[1].1, COOKIE);
    assert_eq!(search(&app, id).await["cached"], 1);
    assert!(upstream.take().is_empty());
    assert_eq!(
        api(
            &app,
            "POST",
            &format!("/api/indexers/{id}/test"),
            Value::Null
        )
        .await["ok"],
        true
    );
    // The test search reuses the session the first search validated: no preflight
    // (docs/session-validation.md). Downloads still check it.
    let seen = upstream.take();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].0.starts_with("/torrents.php?"));
    assert_eq!(
        download(&app, ticket).await,
        (StatusCode::OK, TORRENT.to_vec())
    );
    let seen = upstream.take();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].0, "/index.php");
    assert!(seen[1].0.starts_with("/download.php?id=42"));
    assert!(seen.iter().all(|(_, cookie)| cookie == COOKIE));
    server.abort();
}
#[tokio::test]
async fn cookie_edit_origin_change_disable_and_delete_retire_links_and_restarts_keep_them() {
    let upstream = Upstream::new();
    let (url, server) = upstream.serve().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let engine = Arc::new(Engine::new(path.clone()).await.unwrap());
    let i = engine.put_indexer(stock(&url), false).await.unwrap();
    let router = app(&engine);
    let out = search(&router, &i.id).await;
    let old = link(&out);
    upstream.take();
    // Editing a public/redacted definition preserves its secret on the same origin.
    let public = api(&router, "GET", "/api/indexers", Value::Null).await;
    api(
        &router,
        "PATCH",
        &format!("/api/indexers/{}", i.id),
        json!({"definition":public[0]["definition"]}),
    )
    .await;
    assert_eq!(
        engine.indexer(&i.id).await.unwrap().definition.unwrap()["settings"]["cookie"],
        COOKIE
    );
    // The same cookie on the same origin keeps the permanent link
    // (docs/permanent-download-links.md); a changed cookie retires it.
    assert_eq!(download(&router, old).await.0, StatusCode::OK);
    upstream.take();
    api(
        &router,
        "PATCH",
        &format!("/api/indexers/{}", i.id),
        json!({"definition":{"builtin_id":"musopia","settings":{"cookie":ROTATED}}}),
    )
    .await;
    let out = search(&router, &i.id).await;
    let fresh = link(&out);
    assert_eq!(out["cached"], 0);
    assert!(upstream.take().iter().all(|(_, cookie)| cookie == ROTATED));
    // Moving a saved cookie to another origin without a replacement is rejected atomically.
    let before = std::fs::read(&path).unwrap();
    let response = request(
        &router,
        "PATCH",
        &format!("/api/indexers/{}", i.id),
        json!({"url":"http://other.invalid/"}),
    )
    .await;
    assert_eq!(response.0, StatusCode::BAD_REQUEST);
    assert!(!String::from_utf8_lossy(&response.1).contains(ROTATED));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(download(&router, fresh).await.0, StatusCode::OK);
    upstream.take();
    api(
        &router,
        "PATCH",
        &format!("/api/indexers/{}", i.id),
        json!({"enabled":false}),
    )
    .await;
    assert_eq!(download(&router, fresh).await.0, StatusCode::NOT_FOUND);
    assert!(upstream.take().is_empty());
    api(
        &router,
        "PATCH",
        &format!("/api/indexers/{}", i.id),
        json!({"enabled":true}),
    )
    .await;
    let out = search(&router, &i.id).await;
    let before_restart = link(&out).to_owned();
    drop(router);
    drop(engine);
    let engine = Arc::new(Engine::new(path).await.unwrap());
    let router = app(&engine);
    upstream.take();
    // Permanent links survive a restart (docs/permanent-download-links.md).
    assert_eq!(download(&router, &before_restart).await.0, StatusCode::OK);
    upstream.take();
    let out = search(&router, &i.id).await;
    let before_delete = link(&out);
    api(
        &router,
        "DELETE",
        &format!("/api/indexers/{}", i.id),
        Value::Null,
    )
    .await;
    upstream.take();
    assert_eq!(
        download(&router, before_delete).await.0,
        StatusCode::NOT_FOUND
    );
    assert!(upstream.take().is_empty());
    server.abort();
}
#[tokio::test]
async fn login_and_search_failures_never_become_cached_empty_successes() {
    let upstream = Upstream::new();
    let (url, server) = upstream.serve().await;
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
    let i = engine.put_indexer(stock(&url), false).await.unwrap();
    let app = app(&engine);
    for path in ["/index.php", "/torrents.php"] {
        for failure in ["expired", "refresh", "challenge", "challenge-body"] {
            upstream.fail(path, failure);
            for _ in 0..2 {
                let out = search(&app, &i.id).await;
                assert!(
                    out["results"].as_array().unwrap().is_empty(),
                    "{failure}: {out}"
                );
                assert_eq!(
                    out["errors"].as_array().unwrap().len(),
                    1,
                    "{failure}: {out}"
                );
                assert_eq!(out["cached"], 0);
                let seen = upstream.take();
                assert_eq!(seen.len(), if path == "/index.php" { 1 } else { 2 });
                assert!(seen.iter().all(|(_, cookie)| cookie == COOKIE));
            }
        }
    }
    upstream.fail("/torrents.php", "redirect");
    assert_eq!(
        search(&app, &i.id).await["errors"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(upstream.take().len(), 2);
    upstream.fail("/torrents.php", "empty");
    let empty = search(&app, &i.id).await;
    assert_eq!(empty["errors"], json!([]));
    assert_eq!(empty["results"], json!([]));
    assert_eq!(upstream.take().len(), 2);
    assert_eq!(search(&app, &i.id).await["cached"], 1);
    assert!(upstream.take().is_empty());
    server.abort();
}
#[tokio::test]
async fn download_rechecks_login_and_rejects_foreign_targets_without_credentials() {
    let upstream = Upstream::new();
    let (url, server) = upstream.serve().await;
    let foreign = Upstream::new();
    let (other, other_server) = foreign.serve().await;
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
    for cookie in [Value::Null, json!(""), json!(true), json!("bad\r\nX: y")] {
        let mut bad = stock(&url);
        bad.definition.as_mut().unwrap()["settings"]["cookie"] = cookie;
        assert!(engine.put_indexer(bad, false).await.is_err());
    }
    assert!(engine.indexers().await.is_empty());
    assert!(upstream.take().is_empty());
    let i = engine.put_indexer(stock(&url), false).await.unwrap();
    let app = app(&engine);
    let out = engine.search(vec![i.id.clone()], Default::default()).await;
    assert!(out.errors.is_empty());
    let row = &out.results[0];
    let error = engine
        .download(&i.id, row.source_epoch, &format!("{other}download.php"))
        .await
        .err()
        .unwrap();
    assert_eq!(
        error.to_string(),
        "Only same-origin HTTP downloads are supported"
    );
    assert!(foreign.take().is_empty());
    let out = search(&app, &i.id).await;
    let ticket = link(&out);
    upstream.take();
    upstream.fail("/index.php", "expired");
    let (status, body) = download(&app, ticket).await;
    assert!(!status.is_success());
    assert!(!String::from_utf8_lossy(&body).contains("fixture-private-cookie"));
    assert_eq!(upstream.take(), vec![("/index.php".into(), COOKIE.into())]);
    server.abort();
    other_server.abort();
}

#[tokio::test]
async fn session_refresh_survives_cache_download_edits_and_restart_without_exposing_the_cookie() {
    let upstream = Upstream::new();
    let (url, server) = upstream.serve().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let engine = Arc::new(Engine::new(path.clone()).await.unwrap());
    let i = engine.put_indexer(stock(&url), false).await.unwrap();
    let router = app(&engine);
    upstream.fail("/index.php", "set-cookie");
    let out = search(&router, &i.id).await;
    assert_eq!(out["errors"], json!([]));
    let ticket = link(&out).to_owned();
    let seen = upstream.take();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].1, COOKIE);
    assert_eq!(seen[1].1, "session=new");
    let cached = search(&router, &i.id).await;
    assert_eq!(cached["cached"], 1);
    assert!(upstream.take().is_empty());
    assert_eq!(download(&router, &ticket).await.0, StatusCode::OK);
    assert!(upstream.take().iter().all(|(_, c)| c == "session=new"));
    assert!(!out.to_string().contains("session=new"));
    let public = api(&router, "GET", "/api/indexers", Value::Null).await;
    assert!(!public.to_string().contains("session=new"));
    // The configured seed remains unchanged; durable refreshed-session storage is a separate layer.
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        saved["indexers"][0]["definition"]["settings"]["cookie"],
        COOKIE
    );
    // A cosmetic/settings edit retires tickets but keeps this source's current session.
    *upstream.failure.lock().unwrap() = None;
    api(&router, "PATCH", &format!("/api/indexers/{}", i.id), json!({"name":"Renamed Musopia", "definition":{"builtin_id":"musopia","settings":{"freeleech":false}}})).await;
    assert_eq!(download(&router, &ticket).await.0, StatusCode::NOT_FOUND);
    let out = search(&router, &i.id).await;
    assert_eq!(out["cached"], 0);
    assert!(upstream.take().iter().all(|(_, c)| c == "session=new"));
    let ticket = link(&out).to_owned();
    drop(router);
    drop(engine);
    let engine = Arc::new(Engine::new(path.clone()).await.unwrap());
    let router = app(&engine);
    assert!(upstream.take().is_empty()); // Loading config still does no network work.
                                         // A restart reads the same link key, so the link issued before it still works.
    assert_eq!(download(&router, &ticket).await.0, StatusCode::OK);
    assert!(upstream.take().iter().all(|(_, c)| c == "session=new"));
    let out = search(&router, &i.id).await;
    assert_eq!(out["errors"], json!([]));
    // The download already validated the restored session, so the search sends no
    // preflight (docs/session-validation.md).
    let seen = upstream.take();
    assert_eq!(seen.len(), 1);
    assert!(seen.iter().all(|(_, c)| c == "session=new"));
    // Explicitly changing the seed starts a distinct session.
    api(
        &router,
        "PATCH",
        &format!("/api/indexers/{}", i.id),
        json!({"definition":{"builtin_id":"musopia","settings":{"cookie":ROTATED}}}),
    )
    .await;
    let out = search(&router, &i.id).await;
    assert_eq!(out["errors"], json!([]));
    let seen = upstream.take();
    assert_eq!(seen.len(), 2);
    assert!(seen.iter().all(|(_, c)| c == ROTATED));
    server.abort();
}

#[tokio::test]
async fn invalid_saved_session_rejects_before_network_and_never_falls_back_to_the_seed() {
    let upstream = Upstream::new();
    let (url, server) = upstream.serve().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let engine = Arc::new(Engine::new(path.clone()).await.unwrap());
    let i = engine.put_indexer(stock(&url), false).await.unwrap();
    upstream.fail("/index.php", "set-cookie");
    engine.test(&i.id).await.unwrap();
    drop(engine);
    *upstream.failure.lock().unwrap() = None;
    upstream.take();
    let directory = dir.path().join("config.json.sessions");
    let files: Vec<_> = std::fs::read_dir(&directory)
        .unwrap()
        .map(|f| f.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1);
    let snapshot = std::fs::read(&files[0]).unwrap();
    for invalid in [b"invalid private-fixture-value".to_vec(), vec![b'x'; 65537]] {
        std::fs::write(&files[0], invalid).unwrap();
        let engine = Arc::new(Engine::new(path.clone()).await.unwrap());
        let result = search(&app(&engine), &i.id).await;
        assert_eq!(result["errors"].as_array().unwrap().len(), 1);
        assert!(!result.to_string().contains("private-fixture-value"));
        assert!(upstream.take().is_empty());
    }
    // Restoring the valid private snapshot recovers the rotated session.
    std::fs::write(&files[0], snapshot).unwrap();
    let engine = Arc::new(Engine::new(path).await.unwrap());
    engine.test(&i.id).await.unwrap();
    let seen = upstream.take();
    assert_eq!(seen.len(), 2);
    assert!(seen.iter().all(|(_, c)| c == "session=new"));
    server.abort();
}
