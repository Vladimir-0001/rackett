//! Bundled secrets must not follow a source to another origin unless explicitly re-entered.
use axum::{http::HeaderMap, routing::get, Json, Router};
use rackett::{
    builtin,
    engine::Engine,
    models::{Indexer, SearchQuery},
    AppState,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

type Seen = Arc<Mutex<Vec<String>>>;

/// Loopback Milkie fixture recording every API key it receives.
async fn fixture() -> (String, Seen, tokio::task::JoinHandle<()>) {
    let seen: Seen = Arc::default();
    let capture = seen.clone();
    let router = Router::new().route(
        "/api/v1/torrents",
        get(move |headers: HeaderMap| {
            let capture = capture.clone();
            async move {
                capture.lock().unwrap().push(
                    headers
                        .get("x-milkie-auth")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_owned(),
                );
                Json(json!({"hits":0,"torrents":[]}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, seen, task)
}

fn milkie(url: &str, settings: Value) -> Indexer {
    Indexer {
        id: "fixture".into(),
        name: "Fixture Milkie".into(),
        kind: "cardigann".into(),
        url: url.into(),
        request_delay_ms: 0,
        definition: Some(json!({"builtin_id":"milkie","settings":settings})),
        ..Default::default()
    }
}

fn apikey(i: &Indexer) -> Option<Value> {
    i.definition.as_ref()?["settings"].get("apikey").cloned()
}

async fn call(app: &Router, method: &str, path: &str, body: Value) -> (u16, Value) {
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
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn app(engine: &Arc<Engine>) -> Router {
    rackett::api::router().with_state(AppState {
        engine: engine.clone(),
    })
}

async fn patch(app: &Router, id: &str, body: Value) {
    let (status, v) = call(app, "PATCH", &format!("/api/indexers/{id}"), body).await;
    assert_eq!(status, 200, "{v}");
}

async fn assert_missing_key_without_request(
    engine: &std::sync::Arc<Engine>,
    id: &str,
    seen: &Seen,
) {
    let before = seen.lock().unwrap().len();
    let out = engine.search(vec![id.into()], SearchQuery::default()).await;
    assert!(out.results.is_empty());
    assert!(
        out.errors
            .iter()
            .any(|e| e.message.contains("Milkie requires an API key")),
        "{:?}",
        out.errors
    );
    assert!(engine
        .test(id)
        .await
        .unwrap_err()
        .to_string()
        .contains("Milkie requires an API key"));
    assert_eq!(seen.lock().unwrap().len(), before, "upstream was contacted");
}

#[test]
fn origin_comparison_requires_valid_http_urls_with_equal_effective_origin() {
    for (a, b) in [
        ("https://milkie.test/", "https://milkie.test/mirror/path"),
        ("https://milkie.test/", "https://milkie.test:443/"),
        ("http://milkie.test/", "http://MILKIE.test:80/x"),
        ("http://127.0.0.1:8080/", "http://127.0.0.1:8080"),
    ] {
        assert!(builtin::same_origin(a, b), "{a} {b}");
    }
    for (a, b) in [
        ("https://milkie.test/", "http://milkie.test/"),
        ("https://milkie.test/", "https://mirror.milkie.test/"),
        ("https://milkie.test/", "https://milkie.test:8443/"),
        ("http://127.0.0.1:8080/", "http://localhost:8080/"),
        ("", ""),
        ("not a url", "not a url"),
        ("ftp://milkie.test/", "ftp://milkie.test/"),
        ("file:///tmp/a", "file:///tmp/a"),
        ("https://milkie.test/", ""),
    ] {
        assert!(!builtin::same_origin(a, b), "{a} {b}");
    }
}

#[test]
fn preserve_secrets_only_within_one_valid_origin() {
    let old = milkie("https://milkie.test/", json!({"apikey":"old-secret"}));
    for url in ["https://milkie.test/other/", "https://milkie.test:443/"] {
        for settings in [json!({}), json!({"apikey":""})] {
            let mut new = milkie(url, settings);
            builtin::preserve_secrets(&mut new, &old);
            assert_eq!(apikey(&new), Some(json!("old-secret")), "{url}");
        }
    }
    for url in [
        "http://milkie.test/",
        "https://mirror.test/",
        "https://milkie.test:8443/",
        "",
        "milkie.test",
    ] {
        for settings in [json!({}), json!({"apikey":""})] {
            let mut new = milkie(url, settings.clone());
            builtin::preserve_secrets(&mut new, &old);
            assert_ne!(apikey(&new), Some(json!("old-secret")), "{url}");
        }
        let mut new = milkie(url, json!({"apikey":"new-secret"}));
        builtin::preserve_secrets(&mut new, &old);
        assert_eq!(apikey(&new), Some(json!("new-secret")));
    }
    // An invalid saved origin cannot donate secrets even to an identical string.
    let invalid = milkie("", json!({"apikey":"old-secret"}));
    let mut new = milkie("", json!({}));
    builtin::preserve_secrets(&mut new, &invalid);
    assert_eq!(apikey(&new), None);
}

#[tokio::test]
async fn api_patch_to_new_origin_drops_omitted_redacted_and_blank_secrets() {
    let (a, seen_a, server_a) = fixture().await;
    let (b, seen_b, server_b) = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let engine = Arc::new(Engine::new(path.clone()).await.unwrap());
    let api = app(&engine);
    let (status, v) = call(
        &api,
        "POST",
        "/api/indexers",
        serde_json::to_value(milkie(&a, json!({"apikey":"origin-a-secret"}))).unwrap(),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    let out = engine
        .search(vec![id.clone()], SearchQuery::default())
        .await;
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert_eq!(&*seen_a.lock().unwrap(), &["origin-a-secret"]);

    // Same-origin edits keep the saved key: path change and URL-only/name-only patches.
    patch(&api, &id, json!({"url":format!("{a}mirror/")})).await;
    patch(&api, &id, json!({"url":a.trim_end_matches('/')})).await;
    patch(&api, &id, json!({"name":"Renamed"})).await;
    let (_, public) = call(&api, "GET", "/api/indexers", Value::Null).await;
    assert!(!public.to_string().contains("origin-a-secret"));
    patch(
        &api,
        &id,
        json!({"url":a,"definition":public[0]["definition"]}),
    )
    .await;
    patch(
        &api,
        &id,
        json!({"definition":{"builtin_id":"milkie","settings":{"apikey":""}}}),
    )
    .await;
    assert_eq!(
        apikey(&engine.indexer(&id).await.unwrap()),
        Some(json!("origin-a-secret"))
    );

    // URL-only patch to a new origin: the saved key must not move with it.
    patch(&api, &id, json!({"url":b})).await;
    assert_eq!(apikey(&engine.indexer(&id).await.unwrap()), None);
    assert!(!std::fs::read_to_string(&path)
        .unwrap()
        .contains("origin-a-secret"));
    assert_missing_key_without_request(&engine, &id, &seen_b).await;
    // Moving back does not resurrect the discarded key.
    patch(&api, &id, json!({"url":a})).await;
    assert_eq!(apikey(&engine.indexer(&id).await.unwrap()), None);
    assert_missing_key_without_request(&engine, &id, &seen_a).await;

    // Redacted definition and blank key submitted with a new origin.
    for definition in [
        json!({"builtin_id":"milkie","settings":{}}),
        json!({"builtin_id":"milkie","settings":{"apikey":""}}),
        json!({"builtin_id":"milkie"}),
    ] {
        patch(
            &api,
            &id,
            json!({"url":a,"definition":{"builtin_id":"milkie","settings":{"apikey":"origin-a-secret"}}}),
        )
        .await;
        patch(&api, &id, json!({"url":b,"definition":definition})).await;
        let saved = engine.indexer(&id).await.unwrap();
        assert!(apikey(&saved).is_none_or(|v| v == ""), "{definition}");
        assert!(!serde_json::to_string(&saved)
            .unwrap()
            .contains("origin-a-secret"));
        assert_missing_key_without_request(&engine, &id, &seen_b).await;
    }

    // An explicitly supplied key is accepted at the chosen origin.
    patch(
        &api,
        &id,
        json!({"url":b,"definition":{"builtin_id":"milkie","settings":{"apikey":"origin-b-secret"}}}),
    )
    .await;
    let out = engine
        .search(vec![id.clone()], SearchQuery::default())
        .await;
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert_eq!(&*seen_b.lock().unwrap(), &["origin-b-secret"]);
    assert_eq!(&*seen_a.lock().unwrap(), &["origin-a-secret"]);
    drop(api);
    drop(engine);

    // Restart: persisted state holds only the new-origin key and the API stays redacted.
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("origin-b-secret") && !raw.contains("origin-a-secret"));
    let engine = Arc::new(Engine::new(path.clone()).await.unwrap());
    let api = app(&engine);
    let (_, public) = call(&api, "GET", "/api/indexers", Value::Null).await;
    assert!(!public.to_string().contains("origin-b-secret"));
    assert_eq!(public[0]["url"], b);
    assert_eq!(public[0]["definition"]["builtin_id"], "milkie");
    patch(&api, &id, json!({"url":a})).await;
    assert!(!std::fs::read_to_string(&path)
        .unwrap()
        .contains("origin-b-secret"));
    assert_missing_key_without_request(&engine, &id, &seen_a).await;
    assert_eq!(seen_a.lock().unwrap().len(), 1);
    assert_eq!(seen_b.lock().unwrap().len(), 1);
    server_a.abort();
    server_b.abort();
}

#[tokio::test]
async fn api_patch_scheme_host_and_port_changes_are_new_origins() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
    let api = app(&engine);
    let base = "https://milkie.test/";
    let (status, v) = call(
        &api,
        "POST",
        "/api/indexers",
        serde_json::to_value(milkie(base, json!({}))).unwrap(),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let id = v["id"].as_str().unwrap().to_owned();
    let seed = json!({"url":base,"definition":{"builtin_id":"milkie","settings":{"apikey":"saved-secret"}}});
    for (url, kept) in [
        ("https://milkie.test:443/", true),
        ("https://milkie.test/deeper/path/", true),
        ("http://milkie.test/", false),
        ("https://other.test/", false),
        ("https://milkie.test:8443/", false),
    ] {
        patch(&api, &id, seed.clone()).await;
        patch(&api, &id, json!({"url":url})).await;
        let saved = engine.indexer(&id).await.unwrap();
        assert_eq!(apikey(&saved).is_some(), kept, "{url}");
        assert_eq!(saved.url, url);
    }
    // Invalid URLs remain rejected without touching the saved source.
    patch(&api, &id, seed).await;
    for url in ["", "not a url", "ftp://milkie.test/"] {
        let (status, _) = call(
            &api,
            "PATCH",
            &format!("/api/indexers/{id}"),
            json!({"url":url}),
        )
        .await;
        assert_eq!(status, 400, "{url}");
    }
    let saved = engine.indexer(&id).await.unwrap();
    assert_eq!(saved.url, base);
    assert_eq!(apikey(&saved), Some(json!("saved-secret")));
}
