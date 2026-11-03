use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use rackett::{engine::Engine, store, AppState};
use tower::ServiceExt;
const ORIGIN: &str = "https://rackett.example.ts.net:8453";

#[tokio::test]
async fn trusted_https_origin_preserves_csrf_and_host_checks() {
    let dir = tempfile::tempdir().unwrap();
    let engine = std::sync::Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
    let app =
        rackett::api::router_with_public_url(Some(ORIGIN.into())).with_state(AppState { engine });
    for (host, origin, status) in [
        ("rackett.example.ts.net:8453", ORIGIN, StatusCode::OK),
        (
            "rackett.example.ts.net:8453",
            "https://evil.example",
            StatusCode::FORBIDDEN,
        ),
        (
            "rackett.example.ts.net:8453",
            "http://rackett.example.ts.net:8453",
            StatusCode::FORBIDDEN,
        ),
        ("evil.example", ORIGIN, StatusCode::FORBIDDEN),
        ("rackett.example.ts.net:8454", ORIGIN, StatusCode::FORBIDDEN),
        ("localhost:9127", "http://localhost:9127", StatusCode::OK),
        // Reverse proxies may rewrite Host to their loopback upstream.
        ("127.0.0.1:9127", ORIGIN, StatusCode::OK),
    ] {
        let req = Request::builder()
            .method("PATCH")
            .uri("/api/settings")
            .header("host", host)
            .header("origin", origin)
            .header("content-type", "application/json")
            .body(Body::from("{\"cache_ttl_seconds\":90}"))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(req).await.unwrap().status(),
            status,
            "{host} {origin}"
        );
    }
    let req = Request::builder()
        .method("PATCH")
        .uri("/api/settings")
        .header("host", "rackett.example.ts.net:8453")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    assert_eq!(
        app.oneshot(req).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn public_url_is_validated_and_persisted() {
    for invalid in [
        "http://rackett.example.ts.net",
        "https://user:secret@rackett.example.ts.net",
        "https://rackett.example.ts.net/path",
        "https://rackett.example.ts.net/?key=secret",
        "https://rackett.example.ts.net/#fragment",
        "ftp://example.com",
    ] {
        assert!(store::normalize_public_url(invalid).is_err(), "{invalid}");
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let engine = std::sync::Arc::new(Engine::new(path.clone()).await.unwrap());
    engine.set_public_url(&format!("{ORIGIN}/")).await.unwrap();
    assert_eq!(engine.settings().await.base_url(), ORIGIN);
    let loaded = std::sync::Arc::new(Engine::new(path).await.unwrap());
    assert_eq!(loaded.settings().await.public_url.as_deref(), Some(ORIGIN));
    assert!(loaded
        .set_public_url("http://unsafe.example")
        .await
        .is_err());
    assert_eq!(loaded.settings().await.base_url(), ORIGIN);
}

#[test]
fn remote_download_links_use_explicit_origin() {
    let release = rackett::models::Release {
        guid: "https://source.test/file?key=secret".into(),
        link: "https://source.test/file?key=secret".into(),
        indexer_id: "source".into(),
        ..Default::default()
    };
    let mut response = rackett::models::SearchResponse {
        results: vec![release.clone()],
        ..Default::default()
    };
    rackett::protocol::present_results(
        &mut response,
        &rackett::protocol::Links::new(
            std::sync::Arc::new(rackett::ticket::TicketKey::from_bytes(&[1; 32])),
            Default::default(),
            "integration-key",
            ORIGIN,
        ),
    );
    assert!(response.results[0]
        .link
        .starts_with(&format!("{ORIGIN}/dl/")));
    assert!(!response.results[0].link.contains("secret"));
    assert_eq!(release.link, "https://source.test/file?key=secret");
}
