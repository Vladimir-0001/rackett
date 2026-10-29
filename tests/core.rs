use rackett::{
    engine::Engine,
    models::{Indexer, SearchQuery},
    store,
};
#[tokio::test]
async fn settings_persist_and_validation_does_not_mutate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state/config.json");
    let engine = std::sync::Arc::new(Engine::new(path.clone()).await.unwrap());
    let key = engine.settings().await.api_key;
    assert_eq!(key.len(), 64);
    engine
        .update_settings(serde_json::json!({"cache_ttl_seconds":0,"request_timeout_seconds":2}))
        .await
        .unwrap();
    assert!(engine
        .update_settings(serde_json::json!({"request_timeout_seconds":0}))
        .await
        .is_err());
    assert_eq!(engine.settings().await.request_timeout_seconds, 2);
    let loaded = store::load(&path).await.unwrap();
    assert_eq!(loaded.settings.api_key, key);
    assert_eq!(loaded.settings.cache_ttl_seconds, 0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
#[tokio::test]
async fn corrupt_state_is_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(&path, "broken").unwrap();
    assert!(Engine::new(path.clone()).await.is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "broken");
}
#[tokio::test]
async fn unknown_indexer_returns_explicit_error() {
    let dir = tempfile::tempdir().unwrap();
    let e = std::sync::Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
    let r = e
        .search(vec!["missing".into()], SearchQuery::default())
        .await;
    assert!(r.results.is_empty());
    assert_eq!(r.errors.len(), 1);
}
#[tokio::test]
async fn failed_write_does_not_change_memory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let e = std::sync::Arc::new(Engine::new(path.clone()).await.unwrap());
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(e
        .update_settings(serde_json::json!({"cache_ttl_seconds":9}))
        .await
        .is_err());
    assert_eq!(e.settings().await.cache_ttl_seconds, 120);
}
#[tokio::test]
async fn concurrent_searches_coalesce_and_edits_invalidate() {
    use axum::{routing::get, Router};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let router=Router::new().route("/feed",get(move||{let c=count.clone();async move{c.fetch_add(1,Ordering::SeqCst);tokio::time::sleep(std::time::Duration::from_millis(30)).await;axum::response::Html("<rss version=\"2.0\"><channel><title>Fixture</title><item><title>Linux</title><guid>urn:test:1</guid><enclosure url=\"https://example.com/test.torrent\" length=\"1024\" type=\"application/x-bittorrent\"/></item></channel></rss>")}}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let e = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
    let i = e
        .put_indexer(
            Indexer {
                name: "Fixture".into(),
                url: format!("http://{addr}/feed"),
                request_delay_ms: 0,
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap();
    let q = SearchQuery::default();
    let (a, b) = tokio::join!(e.search(vec![], q.clone()), e.search(vec![], q.clone()));
    assert_eq!(a.results.len(), 1);
    assert_eq!(b.results.len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(a.cached + b.cached, 1);
    e.put_indexer(i, true).await.unwrap();
    assert_eq!(e.search(vec![], q).await.results.len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    server.abort();
}
#[tokio::test]
async fn management_rejects_rebinding_and_cross_origin_changes() {
    use axum::{body::Body, http::Request, routing::get, Router};
    use tower::ServiceExt;
    let app = Router::new()
        .route("/", get(|| async { "ok" }).post(|| async { "ok" }))
        .layer(axum::middleware::from_fn(rackett::api::guard));
    for (host, origin, method, expected) in [
        ("attacker.example", None, "GET", 403),
        (
            "127.0.0.1:9117",
            Some("https://attacker.example"),
            "GET",
            403,
        ),
        ("127.0.0.1:9117", None, "POST", 403),
        ("127.0.0.1:9117", Some("http://127.0.0.1:9117"), "POST", 200),
    ] {
        let mut request = Request::builder()
            .uri("/")
            .method(method)
            .header("host", host);
        if let Some(origin) = origin {
            request = request.header("origin", origin);
        }
        assert_eq!(
            app.clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status()
                .as_u16(),
            expected
        );
    }
}
#[tokio::test]
async fn retries_zero_honored_and_transient_get_retried() {
    use axum::{http::StatusCode, routing::get, Router};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let app = Router::new().route(
        "/",
        get(move || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::Relaxed);
                (StatusCode::SERVICE_UNAVAILABLE, "temporary")
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let e = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
    let mut i = e
        .put_indexer(
            Indexer {
                name: "Retry".into(),
                url: format!("http://{addr}/"),
                request_delay_ms: 0,
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap();
    assert!(e.test(&i.id).await.is_err());
    assert_eq!(count.load(Ordering::Relaxed), 1);
    i.retries = 1;
    let i = e.put_indexer(i, true).await.unwrap();
    assert!(e.test(&i.id).await.is_err());
    assert_eq!(count.load(Ordering::Relaxed), 3);
    server.abort();
}

#[tokio::test]
async fn aggregate_concurrency_is_bounded_and_returns_partial_success_by_deadline() {
    use axum::{routing::get, Router};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let a = active.clone();
    let p = peak.clone();
    let feed="<rss version=\"2.0\"><channel><item><title>Fixture</title><guid>urn:fixture</guid><enclosure url=\"https://example.com/a.torrent\" length=\"1\" type=\"application/x-bittorrent\"/></item></channel></rss>";
    let app = Router::new()
        .route(
            "/hang",
            get(move || async move {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                feed
            }),
        )
        .route(
            "/slow",
            get(move || {
                let a = a.clone();
                let p = p.clone();
                async move {
                    let n = a.fetch_add(1, Ordering::SeqCst) + 1;
                    p.fetch_max(n, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    a.fetch_sub(1, Ordering::SeqCst);
                    feed
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let e = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
    e.update_settings(serde_json::json!({"request_timeout_seconds":1}))
        .await
        .unwrap();
    for n in 0..13 {
        e.put_indexer(
            Indexer {
                name: format!("Source {n}"),
                url: format!("http://{addr}/{}", if n == 12 { "hang" } else { "slow" }),
                request_delay_ms: 0,
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap();
    }
    let start = std::time::Instant::now();
    let r = e.search(vec![], SearchQuery::default()).await;
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
    assert_eq!(r.results.len(), 12);
    assert_eq!(r.errors.len(), 1);
    assert!(peak.load(Ordering::SeqCst) <= 8);
    server.abort();
}

#[tokio::test]
async fn empty_results_cache_has_entry_limit_and_clear_invalidates() {
    use axum::{routing::get, Router};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let app = Router::new().route(
        "/",
        get(move || {
            let c = c.clone();
            async move {
                c.fetch_add(1, Ordering::SeqCst);
                "<rss version=\"2.0\"><channel><title>Empty</title></channel></rss>"
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let e = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
    e.put_indexer(
        Indexer {
            name: "Empty".into(),
            url: format!("http://{addr}/"),
            request_delay_ms: 0,
            ..Default::default()
        },
        false,
    )
    .await
    .unwrap();
    for n in 0..66 {
        let q = SearchQuery {
            params: [("q".into(), format!("query{n}"))].into(),
        };
        let r = e.search(vec![], q).await;
        assert!(r.errors.is_empty());
        assert!(r.results.is_empty());
    }
    assert_eq!(count.load(Ordering::SeqCst), 66);
    assert_eq!(e.cache_entries().await, 64);
    e.clear_cache().await;
    assert_eq!(e.cache_entries().await, 0);
    server.abort();
}

#[tokio::test]
async fn cached_search_does_not_wait_for_an_uncached_request_to_the_same_source() {
    use axum::{routing::get, Router};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tokio::sync::Notify;
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (count, started, resume) = (calls.clone(), entered.clone(), release.clone());
    let router = Router::new().route("/", get(move || {
        let (count, started, resume) = (count.clone(), started.clone(), resume.clone());
        async move {
            if count.fetch_add(1, Ordering::SeqCst) == 1 {
                started.notify_one();
                resume.notified().await;
            }
            "<rss version=\"2.0\"><channel><title>Fixture</title><item><title>Linux</title><guid>urn:test:cached</guid><enclosure url=\"https://example.test/file.torrent\" length=\"1024\" type=\"application/x-bittorrent\"/></item></channel></rss>"
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
    engine
        .put_indexer(
            Indexer {
                name: "Fixture".into(),
                url: format!("http://{addr}/"),
                request_delay_ms: 0,
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap();
    let query = SearchQuery {
        params: [("q".into(), "Linux".into())].into(),
    };
    let primed = engine.search(vec![], query.clone()).await;
    assert!(primed.errors.is_empty());
    assert_eq!(primed.results.len(), 1);
    assert_eq!(primed.cached, 0);
    let background = {
        let engine = engine.clone();
        let mut bypass = query.clone();
        bypass.params.insert("cache".into(), "false".into());
        tokio::spawn(async move { engine.search(vec![], bypass).await })
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    // The upstream stays blocked until AFTER the cached request finishes. This is
    // a scheduling guarantee, not a fragile latency assertion against a sleeping server.
    let cached = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        engine.search(vec![], query),
    )
    .await;
    release.notify_one();
    let fresh = background.await.unwrap();
    assert!(fresh.errors.is_empty());
    assert_eq!(fresh.cached, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let cached = cached.expect("A ready cache hit must not queue behind an upstream request");
    assert!(cached.errors.is_empty());
    assert_eq!(cached.cached, 1);
    assert_eq!(cached.results[0].guid, primed.results[0].guid);
    assert_eq!(
        cached.results[0].source_epoch,
        primed.results[0].source_epoch
    );
    server.abort();
}

#[tokio::test]
async fn ranked_source_candidates_preserve_aggregate_paging_and_first_duplicate() {
    use axum::{routing::get, Router};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    fn page(rows: &[(&str, &str, &str)]) -> String {
        let rows = rows.iter().map(|(id, title, date)| format!("<tr><td class=title><span class=title><a href='/details/{id}'>{title}</a></span></td><td class=action><a href='/{id}.torrent'>Get</a></td><td class=size>1 GB</td><td class=date><time datetime='{date}T00:00:00Z'></time></td></tr>")).collect::<String>();
        format!("<table><tbody>{rows}</tbody></table>")
    }
    let a = page(&[
        ("a", "A", "2026-10-05"),
        ("b", "B", "2026-10-09"),
        ("c", "C", "2026-10-07"),
        ("a", "Duplicate must not replace A", "2026-10-11"),
        ("d", "D", "2026-10-01"),
        ("e", "E", "2026-10-03"),
    ]);
    let b = page(&[
        ("f", "F", "2026-10-02"),
        ("g", "G", "2026-10-08"),
        ("h", "H", "2026-10-06"),
        ("i", "I", "2026-10-04"),
    ]);
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let router = Router::new().fallback(get(move |headers: axum::http::HeaderMap| {
        count.fetch_add(1, Ordering::SeqCst);
        let body = if headers["host"].to_str().unwrap().starts_with("127.") {
            a.clone()
        } else {
            b.clone()
        };
        async { axum::response::Html(body) }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
    for name in ["a", "b"] {
        engine
            .put_indexer(
                Indexer {
                    name: name.into(),
                    kind: "cardigann".into(),
                    url: format!(
                        "http://{}:{}/",
                        if name == "a" {
                            "127.0.0.1"
                        } else {
                            "localhost"
                        },
                        addr.port()
                    ),
                    definition: Some(serde_json::json!({"builtin_id":"acgrip","settings":{}})),
                    request_delay_ms: 0,
                    ..Default::default()
                },
                false,
            )
            .await
            .unwrap();
    }
    for (offset, limit, expected) in [
        (1, 2, vec!["G", "C"]),
        (3, 3, vec!["H", "A", "I"]),
        (0, 10, vec!["B", "G", "C", "H", "A", "I", "E", "F", "D"]),
    ] {
        let q = SearchQuery {
            params: [
                ("offset".into(), offset.to_string()),
                ("limit".into(), limit.to_string()),
            ]
            .into(),
        };
        let fresh = engine.search(vec![], q.clone()).await;
        assert!(fresh.errors.is_empty(), "{:?}", fresh.errors);
        assert_eq!(fresh.cached, 0);
        assert_eq!(
            fresh
                .results
                .iter()
                .map(|r| r.title.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        let before = calls.load(Ordering::SeqCst);
        let cached = engine.search(vec![], q).await;
        assert_eq!(cached.cached, 2);
        assert_eq!(
            cached
                .results
                .iter()
                .map(|r| r.title.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(calls.load(Ordering::SeqCst), before);
    }
    server.abort();
}
