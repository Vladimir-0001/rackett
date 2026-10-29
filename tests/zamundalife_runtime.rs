//! Loopback fixtures for the unchanged pinned Zamunda LIFE JSON definition.
use axum::{routing::get, Router};
use rackett::{
    builtin,
    engine::Engine,
    models::{Indexer, SearchQuery},
};
use serde_json::json;
use std::sync::{Arc, Mutex};

async fn fixture(
    body: Arc<Mutex<String>>,
    status: Arc<Mutex<axum::http::StatusCode>>,
    challenge: Arc<Mutex<bool>>,
    seen: Arc<Mutex<Vec<String>>>,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new().route(
        "/api/torrents",
        get(move |uri: axum::http::Uri| {
            let (body, status, challenge, seen) = (
                body.clone(),
                status.clone(),
                challenge.clone(),
                seen.clone(),
            );
            async move {
                seen.lock().unwrap().push(uri.to_string());
                let mut response = axum::http::Response::builder().status(*status.lock().unwrap());
                if *challenge.lock().unwrap() {
                    response = response.header("cf-mitigated", "challenge");
                }
                response
                    .body(axum::body::Body::from(body.lock().unwrap().clone()))
                    .unwrap()
            }
        }),
    );
    (
        url,
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() }),
    )
}

#[tokio::test]
async fn unchanged_zamundalife_definition_create_restart_test_search_and_fail_closed() {
    let body = Arc::new(Mutex::new(json!([
        {"category":"Филми/HD","title":"Synthetic Film","link":"magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567","size":4096,"description":"Synthetic fixture"}
    ]).to_string()));
    let status = Arc::new(Mutex::new(axum::http::StatusCode::OK));
    let challenge = Arc::new(Mutex::new(false));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (url, server) = fixture(
        body.clone(),
        status.clone(),
        challenge.clone(),
        seen.clone(),
    )
    .await;
    let catalog = builtin::catalog()
        .iter()
        .find(|entry| entry["id"] == "zamundalife")
        .unwrap();
    assert_eq!(catalog["ready"], true);
    assert_eq!(catalog["caps"]["search"], json!(["q"]));
    assert_eq!(catalog["caps"]["tv-search"], json!(["q", "season", "ep"]));
    assert_eq!(catalog["caps"]["movie-search"], json!(["q"]));
    assert_eq!(catalog["caps"]["music-search"], json!(["q"]));
    assert_eq!(catalog["caps"]["book-search"], json!(["q"]));
    for category in [
        2000, 2030, 2040, 2050, 2060, 3020, 4050, 5000, 5060, 5070, 6000, 6070, 7000, 7030, 8000,
    ] {
        assert!(
            catalog["categories"]
                .as_array()
                .unwrap()
                .contains(&json!(category)),
            "missing category {category}"
        );
    }

    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    let engine = Arc::new(Engine::new(config.clone()).await.unwrap());
    let configured = engine
        .put_indexer(
            Indexer {
                name: "Zamunda fixture".into(),
                kind: "cardigann".into(),
                url: url.clone(),
                request_delay_ms: 0,
                definition: Some(json!({"builtin_id":"zamundalife","settings":{}})),
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap();
    drop(engine);
    let engine = std::sync::Arc::new(Engine::new(config).await.unwrap());
    assert!(engine.test(&configured.id).await.is_ok());
    let query = SearchQuery {
        params: [
            ("q".into(), "A.B & C".into()),
            ("cat".into(), "2040".into()),
        ]
        .into_iter()
        .collect(),
    };
    let found = engine.search(vec![configured.id.clone()], query).await;
    assert!(found.errors.is_empty(), "{:?}", found.errors);
    assert_eq!(found.results.len(), 1);
    assert_eq!(found.results[0].title, "Synthetic Film");
    assert_eq!(
        found.results[0].link,
        "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"
    );
    assert_eq!(found.results[0].size, Some(4096));
    assert!(found.results[0].categories.contains(&2040));
    let requested = seen.lock().unwrap().clone();
    assert!(
        requested
            .iter()
            .any(|uri| uri == "/api/torrents?q=A+B+%26+C&limit=50"),
        "{requested:?}"
    );

    *body.lock().unwrap() = "[]".into();
    assert!(engine
        .search(
            vec![configured.id.clone()],
            SearchQuery {
                params: [("cache".into(), "false".into())].into_iter().collect()
            }
        )
        .await
        .errors
        .is_empty());
    assert!(engine.test(&configured.id).await.is_ok());
    for invalid in ["{\"items\":[]}", "not-json"] {
        *body.lock().unwrap() = invalid.into();
        assert!(!engine
            .search(vec![configured.id.clone()], SearchQuery::default())
            .await
            .errors
            .is_empty());
    }
    *body.lock().unwrap() = "fixture private response body".into();
    *status.lock().unwrap() = axum::http::StatusCode::BAD_GATEWAY;
    let error = engine
        .search(
            vec![configured.id.clone()],
            SearchQuery {
                params: [("cache".into(), "false".into())].into_iter().collect(),
            },
        )
        .await
        .errors
        .remove(0)
        .message;
    assert!(error.contains("HTTP 502"));
    assert!(!error.contains("fixture private"));
    *status.lock().unwrap() = axum::http::StatusCode::OK;
    *challenge.lock().unwrap() = true;
    let error = engine
        .search(
            vec![configured.id],
            SearchQuery {
                params: [("cache".into(), "false".into())].into_iter().collect(),
            },
        )
        .await
        .errors
        .remove(0)
        .message;
    assert!(error.contains("Cloudflare challenge detected"));
    server.abort();
}
