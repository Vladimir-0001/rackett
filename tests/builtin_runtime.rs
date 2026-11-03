use axum::{routing::get, Router};
use rackett::{
    builtin,
    engine::Engine,
    models::{Indexer, SearchQuery},
};
use serde_json::json;
use std::sync::{Arc, Mutex};
async fn fixture(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", l.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(l, router).await.unwrap() });
    (url, task)
}
fn stock(id: &str, url: &str) -> Indexer {
    Indexer {
        name: format!("Fixture {id}"),
        kind: "cardigann".into(),
        url: url.into(),
        request_delay_ms: 0,
        definition: Some(json!({"builtin_id":id,"settings":{}})),
        ..Default::default()
    }
}
/// Public layout cases: the original manifest plus independently merged batches.
const PUBLIC_MANIFESTS: [&str; 21] = [
    include_str!("fixtures/public-layouts/cases.json"),
    include_str!("fixtures/public-batches/batch1.json"),
    include_str!("fixtures/public-batches/batch2.json"),
    include_str!("fixtures/public-batches/batch3.json"),
    include_str!("fixtures/public-batches/batch4.json"),
    include_str!("fixtures/public-batches/batch5.json"),
    include_str!("fixtures/public-batches/batch6.json"),
    include_str!("fixtures/public-batches/batch7.json"),
    include_str!("fixtures/public-batches/batch8.json"),
    include_str!("fixtures/public-batches/batch9.json"),
    include_str!("fixtures/public-batches/batch10.json"),
    include_str!("fixtures/public-batches/batch11.json"),
    include_str!("fixtures/public-batches/batch12.json"),
    include_str!("fixtures/public-batches/batch13.json"),
    include_str!("fixtures/public-batches/batch14.json"),
    include_str!("fixtures/public-batches/batch15.json"),
    include_str!("fixtures/public-batches/batch16.json"),
    include_str!("fixtures/public-batches/batch16-thepiratebay.json"),
    include_str!("fixtures/public-batches/batch17.json"),
    include_str!("fixtures/public-batches/batch18.json"),
    include_str!("fixtures/public-batches/batch-oneoffs.json"),
];
/// Cookie layout cases: the original manifest plus independently merged batches.
const COOKIE_MANIFESTS: [&str; 9] = [
    include_str!("fixtures/cookie-layouts/cases.json"),
    include_str!("fixtures/cookie-batches/batch1.json"),
    include_str!("fixtures/cookie-batches/batch2.json"),
    include_str!("fixtures/cookie-batches/batch3.json"),
    include_str!("fixtures/cookie-batches/batch4.json"),
    include_str!("fixtures/cookie-batches/batch5.json"),
    include_str!("fixtures/cookie-batches/batch6.json"),
    include_str!("fixtures/cookie-batches/batch7.json"),
    include_str!("fixtures/cookie-batches/batch8.json"),
];
#[test]
fn catalog_is_embedded_and_conservatively_gated() {
    assert_eq!(builtin::SOURCES.len(), 583);
    use sha2::{Digest, Sha256};
    for (id, raw) in builtin::SOURCES {
        let expected = builtin::CHECKSUMS
            .lines()
            .find_map(|line| {
                line.split_once("  ")
                    .filter(|(_, p)| *p == format!("definitions/{id}.yml"))
                    .map(|(h, _)| h)
            })
            .unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(raw.as_bytes())), expected);
    }
    assert!(builtin::CHECKSUMS.contains("definitions/milkie.yml"));
    let ready: Vec<_> = builtin::catalog()
        .iter()
        .filter(|v| v["ready"] == true)
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    let family: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/cookie-nexusphp/cases.json")).unwrap();
    let api_family: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/unit3d-api/cases.json")).unwrap();
    let mut expected: Vec<_> = family.iter().map(|v| v["id"].as_str().unwrap()).collect();
    expected.extend(api_family.iter().map(|v| v["id"].as_str().unwrap()));
    let nexus_api: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/nexusphp-api/cases.json")).unwrap();
    expected.extend(nexus_api.iter().map(|v| v["id"].as_str().unwrap()));
    let cookie_layouts: Vec<serde_json::Value> = COOKIE_MANIFESTS
        .iter()
        .flat_map(|m| serde_json::from_str::<Vec<serde_json::Value>>(m).unwrap())
        .collect();
    expected.extend(cookie_layouts.iter().map(|v| v["id"].as_str().unwrap()));
    let forms: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/form-layouts/cases.json")).unwrap();
    expected.extend(forms.iter().map(|v| v["id"].as_str().unwrap()));
    let posts: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/post-layouts/cases.json")).unwrap();
    expected.extend(posts.iter().map(|v| v["id"].as_str().unwrap()));
    let batches: Vec<Vec<serde_json::Value>> = [
        include_str!("fixtures/form-batches/batch1.json"),
        include_str!("fixtures/form-batches/batch2.json"),
        include_str!("fixtures/form-batches/batch3.json"),
        include_str!("fixtures/form-batches/batch4.json"),
        include_str!("fixtures/form-batches/batch5.json"),
        include_str!("fixtures/form-batches/batch6.json"),
        include_str!("fixtures/form-batches/batch7.json"),
        include_str!("fixtures/form-batches/batch8.json"),
        include_str!("fixtures/form-batches/batch9.json"),
        include_str!("fixtures/form-batches/batch10.json"),
        include_str!("fixtures/form-batches/batch11.json"),
        include_str!("fixtures/form-batches/batch12.json"),
        include_str!("fixtures/form-batches/batch13.json"),
        include_str!("fixtures/form-batches/batch14.json"),
        include_str!("fixtures/form-batches/batch15.json"),
        include_str!("fixtures/form-batches/batch16.json"),
        include_str!("fixtures/form-batches/batch17.json"),
        include_str!("fixtures/form-batches/batch18.json"),
        include_str!("fixtures/form-batches/batch19.json"),
        include_str!("fixtures/form-batches/batch20.json"),
        include_str!("fixtures/form-batches/batch21.json"),
        include_str!("fixtures/form-batches/batch22.json"),
        include_str!("fixtures/form-batches/batch23.json"),
        include_str!("fixtures/form-batches/batch24.json"),
        include_str!("fixtures/form-batches/batch25.json"),
        include_str!("fixtures/form-batches/batch26.json"),
        include_str!("fixtures/form-batches/batch27.json"),
        include_str!("fixtures/form-batches/batch-oneoffs.json"),
    ]
    .iter()
    .map(|b| serde_json::from_str(b).unwrap())
    .collect();
    expected.extend(batches.iter().flatten().map(|v| v["id"].as_str().unwrap()));
    let public: Vec<serde_json::Value> = PUBLIC_MANIFESTS
        .iter()
        .flat_map(|m| serde_json::from_str::<Vec<serde_json::Value>>(m).unwrap())
        .collect();
    expected.extend(public.iter().map(|v| v["id"].as_str().unwrap()));
    expected.extend([
        "1337x",
        "acgrip",
        "btetree",
        "eztv",
        "milkie",
        "musopia",
        "tokyotosho",
        "uindex",
        "zamundalife",
    ]);
    expected.sort_unstable();
    assert_eq!(ready, expected);
    let acg = builtin::catalog()
        .iter()
        .find(|v| v["id"] == "acgrip")
        .unwrap();
    assert_eq!(acg["categories"], json!([5000]));
    assert_eq!(
        builtin::source_categories(&stock("acgrip", "https://acg.rip/")),
        vec![5000]
    );
    for entry in builtin::catalog().iter().filter(|v| v["ready"] == false) {
        assert!(!entry["blocked_reason"].as_str().unwrap().is_empty());
        assert!(rackett::adapters::validate(&stock(
            entry["id"].as_str().unwrap(),
            "https://example.test/"
        ))
        .is_err());
    }
    assert_eq!(rackett::adapters::catalog().as_array().unwrap().len(), 588);
}
#[tokio::test]
async fn milkie_auth_persist_edit_cache_tickets_and_remove() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let (url,server)=fixture(Router::new().route("/api/v1/torrents",get(move |uri:axum::http::Uri,h:axum::http::HeaderMap|{let seen=seen.clone();async move{seen.lock().unwrap().push((uri.to_string(),h["x-milkie-auth"].to_str().unwrap().to_owned()));axum::Json(json!({"hits":1,"torrents":[{"id":"42","category":1,"releaseName":"Fixture Movie","createdAt":"2026-10-27T00:00:00Z","size":12345,"downloaded":9,"seeders":3,"leechers":2}]}))}}))).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let e = Arc::new(Engine::new(path.clone()).await.unwrap());
    assert!(e.indexers().await.is_empty());
    let mut i = stock("milkie", &url);
    i.definition.as_mut().unwrap()["settings"] = json!({"apikey":"fixture-token & private"});
    let mut i = e.put_indexer(i, false).await.unwrap();
    drop(e);
    let e = Arc::new(Engine::new(path).await.unwrap());
    let q = SearchQuery {
        params: std::collections::BTreeMap::from([
            ("q".into(), "A & B/é".into()),
            ("cat".into(), "2000".into()),
        ]),
    };
    let mut out = e.search(vec![i.id.clone()], q.clone()).await;
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert_eq!(out.results.len(), 1);
    assert_eq!(out.results[0].indexer_id, i.id);
    assert_eq!(out.results[0].indexer, i.name);
    assert_eq!(out.results[0].peers, Some(5));
    assert!(out.results[0].link.contains("key=fixture-token"));
    let links = e
        .links(&out.results, "localkey", "http://127.0.0.1:9117")
        .await;
    rackett::protocol::present_results(&mut out, &links);
    assert!(!serde_json::to_string(&out)
        .unwrap()
        .contains("fixture-token"));
    assert!(!serde_json::to_string(&builtin::public_definition(&i))
        .unwrap()
        .contains("fixture-token"));
    assert_eq!(e.search(vec![i.id.clone()], q.clone()).await.cached, 1);
    assert_eq!(requests.lock().unwrap().len(), 1);
    i.definition.as_mut().unwrap()["settings"] = json!({"apikey":""});
    i = e.put_indexer(i, true).await.unwrap();
    assert_eq!(
        i.definition.as_ref().unwrap()["settings"]["apikey"],
        "fixture-token & private"
    );
    assert_eq!(e.search(vec![i.id.clone()], q).await.results.len(), 1);
    e.test(&i.id).await.unwrap();
    {
        let seen = requests.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert!(seen[0].0.contains("query=A+%26+B%2F%C3%A9"));
        assert_eq!(seen[0].1, "fixture-token & private");
    }
    let mut unsupported = SearchQuery::default();
    unsupported.params.insert("imdbid".into(), "tt42".into());
    assert!(!e
        .search(vec![i.id.clone()], unsupported)
        .await
        .errors
        .is_empty());
    e.delete_indexer(&i.id).await.unwrap();
    assert!(e.indexers().await.is_empty());
    server.abort();
}
#[tokio::test]
async fn acgrip_uindex_and_true_modes() {
    let html="<table><tbody><tr><td class=title><span class=title><a href='/details/1'>Fixture Anime</a></span></td><td class=action><a href='/file.torrent'>Get</a></td><td class=size>1 GB</td><td class=date><time datetime='2026-10-27T00:00:00Z'></time></td></tr></tbody></table>";
    let (url, s) =
        fixture(Router::new().route("/", get(move || async move { axum::response::Html(html) })))
            .await;
    let d = tempfile::tempdir().unwrap();
    let e = Arc::new(Engine::new(d.path().join("config.json")).await.unwrap());
    let i = e.put_indexer(stock("acgrip", &url), false).await.unwrap();
    let out = e.search(vec![], SearchQuery::default()).await;
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert_eq!(out.results.len(), 1);
    assert!(out.results[0].categories.contains(&5000));
    let q = SearchQuery {
        params: std::collections::BTreeMap::from([("t".into(), "movie".into())]),
    };
    assert_eq!(e.search(vec![], q).await.errors.len(), 1);
    e.delete_indexer(&i.id).await.unwrap();
    s.abort();
    let html="<table class=sr-table><tbody><tr><td><a href='/search.php?c=1'>Movies</a><a href='/details.php?id=1'>Fixture Movie</a><a href='magnet:?xt=urn:btih:0123456789012345678901234567890123456789'>Get</a></td><td class=sr-col-uploaded>2026-10-27</td><td class=sr-col-size>2 GB</td><td class=sr-col-seeders>4</td><td class=sr-col-leechers>2</td></tr></tbody></table>";
    let (url, s) = fixture(Router::new().route(
        "/search.php",
        get(move || async move { axum::response::Html(html) }),
    ))
    .await;
    e.put_indexer(stock("uindex", &url), false).await.unwrap();
    let out = e.search(vec![], SearchQuery::default()).await;
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert_eq!(out.results.len(), 1);
    assert!(out.results[0].link.starts_with("magnet:"));
    s.abort();
}
#[tokio::test]
async fn hostile_redirect_does_not_forward_auth_and_errors_are_redacted() {
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = hits.clone();
    let (hostile, h) = fixture(Router::new().fallback(get(move || {
        let seen = seen.clone();
        async move {
            seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            "bad"
        }
    })))
    .await;
    let (url, s) = fixture(Router::new().route(
        "/api/v1/torrents",
        get(move || {
            let target = hostile.clone();
            async move { axum::response::Redirect::temporary(&target) }
        }),
    ))
    .await;
    let d = tempfile::tempdir().unwrap();
    let e = Arc::new(Engine::new(d.path().join("config.json")).await.unwrap());
    let mut i = stock("milkie", &url);
    i.definition.as_mut().unwrap()["settings"] = json!({"apikey":"fixture-secret"});
    i.retries = 3;
    e.put_indexer(i, false).await.unwrap();
    let out = e.search(vec![], SearchQuery::default()).await;
    assert_eq!(out.errors.len(), 1);
    assert!(!serde_json::to_string(&out)
        .unwrap()
        .contains("fixture-secret"));
    assert_eq!(hits.load(std::sync::atomic::Ordering::Relaxed), 0);
    s.abort();
    h.abort();
}

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    body: serde_json::Value,
) -> (axum::http::StatusCode, serde_json::Value) {
    use tower::ServiceExt;
    let request = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1:9127")
        .header("origin", "http://127.0.0.1:9127")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
#[tokio::test]
async fn management_stock_configuration_secret_contract() {
    let d = tempfile::tempdir().unwrap();
    let e = Arc::new(Engine::new(d.path().join("config.json")).await.unwrap());
    let app = rackett::api::router().with_state(rackett::AppState { engine: e.clone() });
    let (status, v) = call(
        &app,
        "POST",
        "/api/indexers",
        serde_json::to_value(stock("milkie", "http://127.0.0.1:9/")).unwrap(),
    )
    .await;
    assert!(status.is_success());
    let id = v["id"].as_str().unwrap();
    let (status,_)=call(&app,"PATCH",&format!("/api/indexers/{id}"),json!({"definition":{"builtin_id":"milkie","settings":{"apikey":"management-private-token"}}})).await;
    assert!(status.is_success());
    let (_, public) = call(&app, "GET", "/api/indexers", json!(null)).await;
    assert!(!public.to_string().contains("management-private-token"));
    assert_eq!(public[0]["definition"]["builtin_id"], "milkie");
    let (status, _) = call(
        &app,
        "PATCH",
        &format!("/api/indexers/{id}"),
        json!({"definition":public[0]["definition"]}),
    )
    .await;
    assert!(status.is_success());
    assert_eq!(
        e.indexer(id).await.unwrap().definition.unwrap()["settings"]["apikey"],
        "management-private-token"
    );
    let (status, _) = call(
        &app,
        "POST",
        "/api/indexers",
        serde_json::to_value(stock("polishtracker-api", "http://127.0.0.1:9/")).unwrap(),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    let (status, _) = call(&app, "DELETE", &format!("/api/indexers/{id}"), json!(null)).await;
    assert!(status.is_success());
}

#[tokio::test]
async fn valid_empty_format_errors_utf8_body_caps_and_deadlines() {
    for (id, path, body, ok) in [
        ("acgrip", "/", "<table><tbody></tbody></table>", true),
        (
            "acgrip",
            "/",
            "<html><body>Challenge required</body></html>",
            false,
        ),
        (
            "uindex",
            "/search.php",
            "<table class=sr-table><tbody></tbody></table>",
            true,
        ),
        (
            "milkie",
            "/api/v1/torrents",
            r#"{"hits":0,"torrents":[]}"#,
            true,
        ),
        (
            "milkie",
            "/api/v1/torrents",
            r#"{"error":"private fixture token"}"#,
            false,
        ),
    ] {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = calls.clone();
        let (url, s) = fixture(Router::new().route(
            path,
            get(move || {
                seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                async move { body }
            }),
        ))
        .await;
        let d = tempfile::tempdir().unwrap();
        let e = Arc::new(Engine::new(d.path().join("c.json")).await.unwrap());
        let mut source = stock(id, &url);
        // Exercise response handling after credential preflight, including error bodies.
        if id == "milkie" {
            source.definition.as_mut().unwrap()["settings"]["apikey"] = json!("fixture-key");
        }
        e.put_indexer(source, false).await.unwrap();
        let out = e.search(vec![], SearchQuery::default()).await;
        assert_eq!(out.errors.is_empty(), ok, "{id} {body}: {:?}", out.errors);
        assert!(out.results.is_empty());
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        s.abort();
    }
    for response in [
        axum::http::Response::builder()
            .header("content-type", "text/html; charset=iso-8859-1")
            .body(axum::body::Body::from("<table><tbody></tbody></table>"))
            .unwrap(),
        axum::http::Response::builder()
            .header("content-length", "8388609")
            .body(axum::body::Body::from(vec![b'x'; 8388609]))
            .unwrap(),
        axum::http::Response::builder()
            .body(axum::body::Body::from(vec![255u8]))
            .unwrap(),
    ] {
        let response = Arc::new(tokio::sync::Mutex::new(Some(response)));
        let (url, s) = fixture(Router::new().route(
            "/",
            get(move || {
                let r = response.clone();
                async move { r.lock().await.take().unwrap() }
            }),
        ))
        .await;
        let d = tempfile::tempdir().unwrap();
        let e = Arc::new(Engine::new(d.path().join("c.json")).await.unwrap());
        e.put_indexer(stock("acgrip", &url), false).await.unwrap();
        assert_eq!(
            e.search(vec![], SearchQuery::default()).await.errors.len(),
            1
        );
        s.abort();
    }
    let (url, s) = fixture(Router::new().route(
        "/",
        get(|| async {
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            "<table><tbody></tbody></table>"
        }),
    ))
    .await;
    let d = tempfile::tempdir().unwrap();
    let e = Arc::new(Engine::new(d.path().join("c.json")).await.unwrap());
    e.update_settings(json!({"request_timeout_seconds":1}))
        .await
        .unwrap();
    e.put_indexer(stock("acgrip", &url), false).await.unwrap();
    let q = SearchQuery::default();
    let now = std::time::Instant::now();
    let (a, b) = tokio::join!(e.search(vec![], q.clone()), e.search(vec![], q));
    assert_eq!(a.errors.len() + b.errors.len(), 2);
    assert!(now.elapsed() < std::time::Duration::from_secs(2));
    s.abort();
}

#[tokio::test]
async fn executable_serves_embedded_catalog_outside_repository() {
    let outside = tempfile::tempdir().unwrap();
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    drop(socket);
    let mut process = tokio::process::Command::new(env!("CARGO_BIN_EXE_rackett"))
        .current_dir(outside.path())
        .args(["--port", &port.to_string(), "--data-dir", "state"])
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}");
    let mut ready = false;
    for _ in 0..3600 {
        if client.get(format!("{url}/health")).send().await.is_ok() {
            ready = true;
            break;
        }
        if let Some(status) = process.try_wait().unwrap() {
            panic!("Fixture binary exited {status}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(ready);
    let catalog: serde_json::Value = client
        .get(format!("{url}/api/catalog"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(catalog.as_array().unwrap().len(), 588);
    assert_eq!(
        catalog
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["ready"] == true)
            .count(),
        builtin::READY_IDS.len()
    );
    let sources: serde_json::Value = client
        .get(format!("{url}/api/indexers"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(sources.as_array().unwrap().is_empty());
    process.kill().await.unwrap();
}

#[tokio::test]
async fn stock_ticket_is_one_private_same_origin_fetch_and_caps_are_exact() {
    use tower::ServiceExt;
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = count.clone();
    let router=Router::new().route("/api/v1/torrents",get(||async{axum::Json(json!({"hits":1,"torrents":[{"id":"42","category":1,"releaseName":"Private download fixture","createdAt":"2026-10-27T00:00:00Z","size":123,"downloaded":1,"seeders":1,"leechers":0}]}))})).route("/api/v1/torrents/42/torrent",get(move|uri:axum::http::Uri|{let c=captured.clone();async move{assert!(uri.query().unwrap().contains("key=fixture-download-token"));c.fetch_add(1,std::sync::atomic::Ordering::Relaxed);b"d4:infod4:name6:ubuntu6:lengthi42eee".as_slice()}}));
    let router = router.route(
        "/redirect-download",
        get(|| async {
            (
                axum::http::StatusCode::FOUND,
                [("location", "/api/v1/torrents/42/torrent")],
                b"d4:infod4:name6:ubuntu6:lengthi42eee".as_slice(),
            )
        }),
    );
    let (url, s) = fixture(router).await;
    let d = tempfile::tempdir().unwrap();
    let e = Arc::new(Engine::new(d.path().join("c.json")).await.unwrap());
    let mut i = stock("milkie", &url);
    i.definition.as_mut().unwrap()["settings"] = json!({"apikey":"fixture-download-token"});
    e.put_indexer(i, false).await.unwrap();
    let mut out = e.search(vec![], SearchQuery::default()).await;
    assert_eq!(out.results.len(), 1);
    let key = e.settings().await.api_key;
    let links = e.links(&out.results, &key, "http://127.0.0.1:9127").await;
    rackett::protocol::present_results(&mut out, &links);
    let u = url::Url::parse(&out.results[0].link).unwrap();
    assert!(!u.as_str().contains("fixture-download-token"));
    let app = rackett::protocol::router().with_state(rackett::AppState { engine: e.clone() });
    let req = axum::http::Request::builder()
        .uri(&u[url::Position::BeforePath..])
        .body(axum::body::Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert!(response.status().is_success());
    let bytes = axum::body::to_bytes(response.into_body(), 1000)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), b"d4:infod4:name6:ubuntu6:lengthi42eee");
    assert_eq!(count.load(std::sync::atomic::Ordering::Relaxed), 1);
    let source = e
        .indexers()
        .await
        .into_iter()
        .find(|i| i.kind == "cardigann")
        .unwrap();
    let error = e
        .download(
            &source.id,
            out.results[0].source_epoch,
            &format!("{url}redirect-download"),
        )
        .await
        .unwrap_err();
    assert!(!error.is::<rackett::engine::DownloadExpired>());
    assert_eq!(count.load(std::sync::atomic::Ordering::Relaxed), 1);
    let i = e.put_indexer(stock("acgrip", &url), false).await.unwrap();
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/torznab/{}/api?t=caps&apikey={key}", i.id))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let text = String::from_utf8(
        axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(text.contains("<category id=\"5000\""));
    assert!(text.contains("<movie-search available=\"no\""));
    assert!(text.contains("<tv-search available=\"yes\" supportedParams=\"q,season,ep\""));
    s.abort();
}

// Synthetic response shapes derived from the unchanged pinned field selectors, not live captures.
#[tokio::test]
async fn btetree_management_restart_search_and_downloads() {
    use tower::ServiceExt;
    for (preset, path, body, category, title) in [(
        "btetree",
        "/",
        include_str!("fixtures/builtin/btetree.html"),
        3040,
        "Fixture Ensemble Live FLAC",
    )] {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = requests.clone();
        let downloads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fetched = downloads.clone();
        let (url, server) = fixture(
            Router::new()
                .route(
                    path,
                    get(move |uri: axum::http::Uri| {
                        let seen = seen.clone();
                        async move {
                            seen.lock().unwrap().push(uri.to_string());
                            axum::response::Html(body)
                        }
                    }),
                )
                .route(
                    "/download.php/42/show.torrent",
                    get(move || {
                        fetched.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        async { b"d4:infod4:name6:ubuntu6:lengthi42eee".as_slice() }
                    }),
                ),
        )
        .await;
        let d = tempfile::tempdir().unwrap();
        let config = d.path().join("c.json");
        let e = Arc::new(Engine::new(config.clone()).await.unwrap());
        let app = rackett::api::router().with_state(rackett::AppState { engine: e.clone() });
        let (status, created) = call(
            &app,
            "POST",
            "/api/indexers",
            serde_json::to_value(stock(preset, &url)).unwrap(),
        )
        .await;
        assert!(status.is_success(), "{created}");
        let id = created["id"].as_str().unwrap().to_owned();
        let (_, catalog) = call(&app, "GET", "/api/catalog", json!(null)).await;
        let entry = catalog
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == preset)
            .unwrap();
        assert_eq!(entry["ready"], true);
        assert!(entry["categories"]
            .as_array()
            .unwrap()
            .contains(&json!(category)));
        let settings = json!({"sort":"seeders"});
        let (status, edited) = call(
            &app,
            "PATCH",
            &format!("/api/indexers/{id}"),
            json!({"name":"Edited fixture","definition":{"builtin_id":preset,"settings":settings}}),
        )
        .await;
        assert!(status.is_success(), "{edited}");
        drop(app);
        drop(e);
        let e = Arc::new(Engine::new(config).await.unwrap());
        let app = rackett::api::router().with_state(rackett::AppState { engine: e.clone() });
        assert_eq!(e.indexer(&id).await.unwrap().name, "Edited fixture");
        let (_, tested) = call(
            &app,
            "POST",
            &format!("/api/indexers/{id}/test"),
            json!(null),
        )
        .await;
        assert_eq!(tested["ok"], true, "{tested}");
        let query = format!("/api/search?indexers={id}&q=Fixture&cat={category}");
        let (status, out) = call(&app, "GET", &query, json!(null)).await;
        assert!(status.is_success(), "{out}");
        assert_eq!(out["errors"], json!([]), "{out}");
        let rows = out["results"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{out}");
        let r = &rows[0];
        assert_eq!(r["title"], title);
        assert_eq!(r["indexer"], "Edited fixture");
        assert_eq!(r["size"], 1073741824u64);
        assert_eq!(r["seeders"], 4);
        assert_eq!(r["peers"], 6);
        assert_eq!(r["attributes"]["grabs"], "17");
        assert!(r["published"]
            .as_str()
            .unwrap()
            .ends_with("-10-26T19:30:00Z"));
        assert!(r["guid"].as_str().unwrap().starts_with("urn:rackett:"));
        assert!(r["details"].is_null(), "details query is redacted");
        assert!(r["categories"]
            .as_array()
            .unwrap()
            .contains(&json!(category)));
        assert_eq!(r["attributes"]["downloadvolumefactor"], "0");
        assert_eq!(r["attributes"]["uploadvolumefactor"], "1");
        {
            assert_eq!(r["attributes"]["files"], "12");
            let link = r["link"].as_str().unwrap();
            assert!(!link.contains("download.php"), "must expose opaque ticket");
            let link = url::Url::parse(link).unwrap();
            let protocol =
                rackett::protocol::router().with_state(rackett::AppState { engine: e.clone() });
            let response = protocol
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .uri(&link[url::Position::BeforePath..])
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert!(response.status().is_success());
            assert_eq!(
                axum::body::to_bytes(response.into_body(), 1000)
                    .await
                    .unwrap()
                    .as_ref(),
                b"d4:infod4:name6:ubuntu6:lengthi42eee"
            );
            assert_eq!(downloads.load(std::sync::atomic::Ordering::Relaxed), 1);
        }
        let seen = requests.lock().unwrap().clone();
        assert_eq!(seen.len(), 2);
        let u = url::Url::parse(&format!("{url}{}", seen[1].trim_start_matches('/'))).unwrap();
        let params: std::collections::BTreeMap<_, _> = u.query_pairs().collect();
        assert_eq!(params.get("searchzzzz").unwrap(), "Fixture");
        assert_eq!(params.get("sort").unwrap(), "seeders");
        assert_eq!(params.get("cat").unwrap(), "0");
        let protocol =
            rackett::protocol::router().with_state(rackett::AppState { engine: e.clone() });
        let key = e.settings().await.api_key;
        let response = protocol
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/torznab/{id}/api?t=caps&apikey={key}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response.status().is_success());
        let caps = String::from_utf8(
            axum::body::to_bytes(response.into_body(), 20000)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(caps.contains(&format!("<category id=\"{category}\"")));
        assert!(caps.contains("<music-search available=\"yes\" supportedParams=\"q,artist\""));
        assert!(caps.contains("<tv-search available=\"no\""));
        let (_, music) = call(
            &app,
            "GET",
            &format!("/api/search?indexers={id}&t=music&artist=Fixture"),
            json!(null),
        )
        .await;
        assert_eq!(music["results"].as_array().unwrap().len(), 1, "{music}");
        assert!(requests
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .contains("searchzzzz=Fixture"));
        let (_, filtered) = call(
            &app,
            "GET",
            &format!("/api/search?indexers={id}&q=Unmatched"),
            json!(null),
        )
        .await;
        assert_eq!(filtered["results"], json!([]));
        assert_eq!(filtered["errors"], json!([]));
        let before = requests.lock().unwrap().len();
        let (status, invalid) = call(
            &app,
            "GET",
            &format!("/api/search?indexers={id}&t=movie&imdbid=tt1234567"),
            json!(null),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
        assert!(invalid["error"].as_str().unwrap().contains("unsupported"));
        assert_eq!(requests.lock().unwrap().len(), before);
        let (_, public) = call(&app, "GET", "/api/indexers", json!(null)).await;
        assert_eq!(public[0]["definition"]["builtin_id"], preset);
        let (status, _) = call(&app, "DELETE", &format!("/api/indexers/{id}"), json!(null)).await;
        assert!(status.is_success());
        server.abort();
    }
}

#[tokio::test]
async fn btetree_empty_malformed_http_errors_and_encoded_requests() {
    for (preset, path, empty) in [(
        "btetree",
        "/",
        "<table bgcolor='#CCCCCC'><tbody></tbody></table>",
    )] {
        let malformed = include_str!("fixtures/builtin/btetree.html")
            .replace("class=\"details_link\"", "class=\"broken\"");
        for (body, status, ok) in [
            (empty, axum::http::StatusCode::OK, true),
            (
                "<table><tr><td>Challenge private-token</td></tr></table>",
                axum::http::StatusCode::OK,
                false,
            ),
            (malformed.as_str(), axum::http::StatusCode::OK, false),
            (
                "<html>Challenge private-token</html>",
                axum::http::StatusCode::OK,
                false,
            ),
            ("private-token", axum::http::StatusCode::FORBIDDEN, false),
        ] {
            let body = body.to_owned();
            let seen = Arc::new(Mutex::new(Vec::new()));
            let captured = seen.clone();
            let (url, server) = fixture(Router::new().route(
                path,
                get(move |uri: axum::http::Uri| {
                    captured.lock().unwrap().push(uri.to_string());
                    let body = body.clone();
                    async move { (status, body) }
                }),
            ))
            .await;
            let d = tempfile::tempdir().unwrap();
            let e = Arc::new(Engine::new(d.path().join("c.json")).await.unwrap());
            e.put_indexer(stock(preset, &url), false).await.unwrap();
            let out = e
                .search(
                    vec![],
                    SearchQuery {
                        params: std::collections::BTreeMap::from([("q".into(), "A & B/é".into())]),
                    },
                )
                .await;
            assert_eq!(out.errors.is_empty(), ok, "{preset}: {:?}", out.errors);
            assert!(out.results.is_empty());
            assert!(!serde_json::to_string(&out)
                .unwrap()
                .contains("private-token"));
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 1);
            assert!(seen[0].contains("A+%26+B%2F%C3%A9"), "{}", seen[0]);
            server.abort();
        }
    }
}
