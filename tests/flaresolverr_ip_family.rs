//! One test per process: solver environment is read once. Real loopback sockets
//! prove the configured family affects only admitted source clients, not the solver
//! connection or native adapters. No real source, credentials or solver is used.
use axum::{http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use rackett::{engine::Engine, models::Indexer};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
const PAGE: &str =
    "<html><table class=forum_header_border><tr><td><h1>Latest</h1></td></tr></table></html>";
const RSS: &str = "<rss><channel><item><title>Fixture</title><guid>fixture</guid><enclosure url='/fixture.torrent' length='42' type='application/x-bittorrent'/></item></channel></rss>";

#[tokio::test]
async fn ipv4_source_pin_keeps_native_and_solver_ipv6_connections_working() {
    let challenge = Arc::new(AtomicBool::new(false));
    let ipv4_hits = Arc::new(AtomicUsize::new(0));
    let ipv6_hits = Arc::new(AtomicUsize::new(0));
    let solves = Arc::new(AtomicUsize::new(0));
    let listener4 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listener6 = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
    let base4 = format!("http://{}/", listener4.local_addr().unwrap());
    let base6 = format!("http://{}/", listener6.local_addr().unwrap());
    let (ch, calls) = (challenge.clone(), ipv4_hits.clone());
    let app4 = Router::new().fallback(move || {
        let (ch, calls) = (ch.clone(), calls.clone());
        async move {
            calls.fetch_add(1, Ordering::SeqCst);
            if ch.load(Ordering::SeqCst) {
                (
                    StatusCode::FORBIDDEN,
                    [("cf-mitigated", "challenge")],
                    "challenge",
                )
                    .into_response()
            } else {
                axum::response::Html(PAGE).into_response()
            }
        }
    });
    let (calls, solver_calls) = (ipv6_hits.clone(), solves.clone());
    let app6 = Router::new().route("/v1", post(move |Json(body): Json<Value>| {
        let calls = solver_calls.clone();
        async move {
            calls.fetch_add(1, Ordering::SeqCst);
            Json(json!({"status":"ok", "solution":{"url":body["url"], "status":200,"response":PAGE,"cookies":[],"userAgent":"Fixture/1"}}))
        }
    })).fallback(move || {
        let calls = calls.clone();
        async move { calls.fetch_add(1, Ordering::SeqCst); RSS }
    });
    let task4 = tokio::spawn(async move { axum::serve(listener4, app4).await.unwrap() });
    let task6 = tokio::spawn(async move { axum::serve(listener6, app6).await.unwrap() });
    std::env::set_var("RACKETT_FLARESOLVERR_URL", &base6);
    std::env::set_var("RACKETT_FLARESOLVERR_SOURCE_IP_FAMILY", "ipv4");
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
    let preset = |url: String| Indexer {
        name: "Fixture EZTV".into(),
        kind: "cardigann".into(),
        url,
        definition: Some(json!({"builtin_id":"eztv","settings":{}})),
        ..Default::default()
    };
    let source4 = engine.put_indexer(preset(base4), false).await.unwrap();
    engine.test(&source4.id).await.unwrap();
    assert_eq!(ipv4_hits.load(Ordering::SeqCst), 1);
    challenge.store(true, Ordering::SeqCst);
    engine.test(&source4.id).await.unwrap();
    assert_eq!(
        solves.load(Ordering::SeqCst),
        1,
        "IPv4 pin must not restrict the IPv6 solver endpoint"
    );
    let source6 = engine
        .put_indexer(preset(base6.clone()), false)
        .await
        .unwrap();
    assert!(engine
        .test(&source6.id)
        .await
        .unwrap_err()
        .to_string()
        .contains("transport failed"));
    assert_eq!(
        ipv6_hits.load(Ordering::SeqCst),
        0,
        "Pinned source must not send over IPv6"
    );
    let native = engine
        .put_indexer(
            Indexer {
                name: "Native IPv6".into(),
                kind: "rss".into(),
                url: base6,
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap();
    engine.test(&native.id).await.unwrap();
    assert_eq!(
        ipv6_hits.load(Ordering::SeqCst),
        1,
        "Native adapter must retain IPv6"
    );
    task4.abort();
    task6.abort();
}
