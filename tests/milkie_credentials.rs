use axum::{http::HeaderMap, routing::get, Json, Router};
use rackett::{
    engine::Engine,
    models::{Indexer, SearchQuery},
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

async fn fixture(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, task)
}

fn milkie(url: &str, settings: Value) -> Indexer {
    Indexer {
        name: "Fixture Milkie".into(),
        kind: "cardigann".into(),
        url: url.into(),
        request_delay_ms: 0,
        definition: Some(json!({"builtin_id":"milkie", "settings":settings})),
        ..Default::default()
    }
}

#[tokio::test]
async fn requires_key_before_request_and_keeps_invalid_source_editable() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let capture = seen.clone();
    let (url, server) = fixture(Router::new().route(
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
    ))
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let engine = Arc::new(Engine::new(path.clone()).await.unwrap());

    let missing = engine
        .put_indexer(milkie(&url, json!({})), false)
        .await
        .unwrap();
    let empty = engine
        .put_indexer(milkie(&url, json!({"apikey":""})), false)
        .await
        .unwrap();
    let whitespace = engine
        .put_indexer(milkie(&url, json!({"apikey":"   "})), false)
        .await
        .unwrap();
    for indexer in [&missing, &empty, &whitespace] {
        let out = engine
            .search(vec![indexer.id.clone()], SearchQuery::default())
            .await;
        assert!(out.results.is_empty());
        assert!(out
            .errors
            .iter()
            .any(|e| e.message.contains("Milkie requires an API key")));
    }
    assert!(engine
        .test(&missing.id)
        .await
        .unwrap_err()
        .to_string()
        .contains("Milkie requires an API key"));
    assert!(seen.lock().unwrap().is_empty());
    drop(engine);

    // A saved source with an invalid key survives startup and remains editable.
    let engine = Arc::new(Engine::new(path).await.unwrap());
    let mut editable = engine.indexer(&missing.id).await.unwrap();
    editable.definition.as_mut().unwrap()["settings"]["apikey"] = json!("fixture key & bytes");
    editable = engine.put_indexer(editable, true).await.unwrap();
    editable.definition.as_mut().unwrap()["settings"] = json!({});
    editable = engine.put_indexer(editable, true).await.unwrap();
    assert_eq!(
        editable.definition.as_ref().unwrap()["settings"]["apikey"],
        "fixture key & bytes"
    );
    editable.definition.as_mut().unwrap()["settings"]["apikey"] = json!("");
    editable = engine.put_indexer(editable, true).await.unwrap();
    assert_eq!(
        editable.definition.as_ref().unwrap()["settings"]["apikey"],
        "fixture key & bytes"
    );
    let out = engine
        .search(vec![editable.id], SearchQuery::default())
        .await;
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert_eq!(&*seen.lock().unwrap(), &["fixture key & bytes"]);
    server.abort();
}
