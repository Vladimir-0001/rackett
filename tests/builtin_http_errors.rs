use axum::{http::StatusCode, routing::get, Router};
use rackett::{
    engine::Engine,
    models::{Indexer, SearchQuery},
};
use serde_json::json;

#[tokio::test]
async fn bundled_http_errors_explain_failures_without_exposing_upstream_data() {
    for (code, cloudflare, challenge, expected) in [
        (401, false, false, "Authentication required"),
        (403, true, true, "Cloudflare challenge detected"),
        // Cloudflare also proxies ordinary forbidden responses: its server header
        // alone does not establish that a challenge caused the failure.
        (403, true, false, "Access denied by the source"),
        (404, false, false, "Search endpoint not found"),
        (429, false, false, "Rate limit exceeded"),
        (503, false, false, "The source is unavailable or failing"),
        (418, false, false, "The source rejected the search request"),
        // A challenge must not reach the parser even on a successful HTTP status.
        (200, true, true, "Cloudflare challenge detected"),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let app = Router::new().route(
            "/api/v1/torrents",
            get(move || async move {
                (
                    StatusCode::from_u16(code).unwrap(),
                    [
                        (
                            "server",
                            if cloudflare {
                                "cloudflare"
                            } else {
                                "fixture-server"
                            },
                        ),
                        (
                            "cf-mitigated",
                            if challenge {
                                "challenge"
                            } else {
                                "private-header"
                            },
                        ),
                    ],
                    "private-body private-key private-query",
                )
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let engine =
            std::sync::Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
        let source = engine
            .put_indexer(
                Indexer {
                    name: "HTTP error fixture".into(),
                    kind: "cardigann".into(),
                    url: url.clone(),
                    definition: Some(
                        json!({"builtin_id":"milkie", "settings":{"apikey":"private-key"}}),
                    ),
                    ..Default::default()
                },
                false,
            )
            .await
            .unwrap();
        let query = SearchQuery {
            params: [("q".into(), "private-query".into())].into(),
        };
        let response = engine.search(vec![source.id.clone()], query).await;
        assert!(response.results.is_empty());
        assert_eq!(response.errors.len(), 1);
        let error = &response.errors[0].message;
        assert!(error.contains(&format!("HTTP {code}")), "{error}");
        assert!(error.contains(expected), "{error}");
        if !challenge {
            assert!(!error.contains("challenge"), "{error}");
        }
        for private in [
            "private-body",
            "private-key",
            "private-query",
            "private-header",
            &url,
        ] {
            assert!(!error.contains(private), "{error}");
        }
        let statuses = engine.statuses.read().await;
        let status = &statuses[&source.id];
        assert_eq!(status.status, "error");
        assert_eq!(status.last_error.as_deref(), Some(error.as_str()));
        server.abort();
    }
}
