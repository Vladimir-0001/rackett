//! Source-pinned NexusPHP v1.9 API fixtures, with no live accounts.
use rackett::{
    builtin,
    cardigann::{self, Node},
    cardigann_exec::Plan,
    models::SearchQuery,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
#[derive(Clone, Deserialize)]
struct Case {
    id: String,
    sha256: String,
    tracker_category: u32,
    category: u32,
    minimumratio: Value,
    minimumseedtime: Value,
}
fn cases() -> Vec<Case> {
    serde_json::from_str(include_str!("fixtures/nexusphp-api/cases.json")).unwrap()
}
fn root(c: &Case) -> Node {
    let raw = builtin::SOURCES
        .iter()
        .find(|(id, _)| *id == c.id)
        .unwrap()
        .1;
    assert_eq!(
        format!("{:x}", Sha256::digest(raw.as_bytes())),
        c.sha256,
        "{}",
        c.id
    );
    cardigann::parse(raw, cardigann::Limits::default()).unwrap()
}
fn settings() -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("apikey".into(), json!("fixture-api-key")),
        ("passkey".into(), json!("fixture-download-secret")),
    ])
}
fn payload(c: &Case, down: f64, up: f64, hr: u8) -> Value {
    json!({"data":{"data":[{"id":42,"category":c.tracker_category,"name":"Fixture Release","small_descr":"Fixture description","size":1073741824,"numfiles":3,"seeders":11,"leechers":3,"times_completed":17,"hash":"0123456789abcdef0123456789abcdef01234567","cover":"https://fixture.invalid/poster","added":"2026-10-28 10:30","promotion_info":{"down_multiplier":down,"up_multiplier":up},"hr":hr}]}})
}
#[test]
fn pinned_fields_promotions_ratio_and_hr_variants_are_extracted() {
    for c in cases() {
        let p = Plan::compile(&root(&c)).unwrap();
        let q = SearchQuery::default();
        let ctx = p.context(&settings(), &q, "2026-10-28T12:00:00Z".parse().unwrap());
        let request = p.build_requests(&ctx).unwrap().remove(0);
        for hr in [0, 1] {
            for (down, up) in [(1., 1.), (0., 2.), (0.5, 1.)] {
                let rows = p
                    .extract(&request, &payload(&c, down, up, hr).to_string(), &ctx)
                    .unwrap_or_else(|e| panic!("{}: {e}", c.id));
                assert_eq!(rows.len(), 1, "{}", c.id);
                let r = &rows[0];
                assert_eq!(r.title, "Fixture Release");
                assert_eq!(r.size, Some(1073741824));
                assert_eq!(r.seeders, Some(11));
                assert_eq!(r.peers, Some(14));
                assert!(r.categories.contains(&c.category), "{}", c.id);
                assert_eq!(r.published.as_deref(), Some("2026-10-28T02:30:00Z"));
                for (key, value) in [
                    ("downloadvolumefactor", down.to_string()),
                    ("uploadvolumefactor", up.to_string()),
                    ("files", "3".into()),
                    ("grabs", "17".into()),
                ] {
                    assert_eq!(r.attributes.get(key), Some(&value), "{} {key}", c.id);
                }
                for (key, value) in [
                    ("minimumratio", &c.minimumratio),
                    ("minimumseedtime", &c.minimumseedtime),
                ] {
                    let expected = if value.is_object() {
                        value.get(hr.to_string()).unwrap().as_str()
                    } else {
                        value.as_str()
                    };
                    if let Some(expected) = expected {
                        let actual = r
                            .attributes
                            .get(key)
                            .unwrap_or_else(|| panic!("{} {key}", c.id));
                        assert_eq!(
                            actual.parse::<f64>().unwrap(),
                            expected.parse::<f64>().unwrap(),
                            "{} {key}",
                            c.id
                        );
                    } else {
                        assert!(!r.attributes.contains_key(key), "{} {key}", c.id);
                    }
                }
                assert!(r
                    .link
                    .ends_with("download.php?id=42&passkey=fixture-download-secret"));
                assert!(r
                    .details
                    .as_deref()
                    .unwrap()
                    .ends_with("details.php?id=42&hit=1"));
            }
        }
    }
}
#[test]
fn pinned_requests_and_freeleech_row_filter_keep_nexus_semantics() {
    for c in cases() {
        let p = Plan::compile(&root(&c)).unwrap();
        let mut settings = settings();
        settings.insert("freeleech".into(), json!(true));
        settings.insert("sort".into(), json!("-seeders"));
        let q = SearchQuery {
            params: BTreeMap::from([
                ("q".into(), "Fixture..Release".into()),
                ("cat".into(), c.category.to_string()),
            ]),
        };
        let ctx = p.context(&settings, &q, chrono::Utc::now());
        let requests = p.build_requests(&ctx).unwrap();
        assert_eq!(requests.len(), 1);
        let r = &requests[0];
        assert_eq!(r.method, "GET");
        assert!(r.body.is_none());
        assert_eq!(
            r.headers,
            [("Authorization".into(), "Bearer fixture-api-key".into())]
        );
        let url = url::Url::parse(&r.url).unwrap();
        assert_eq!(url.path(), "/api/v1/torrents");
        let pairs: Vec<_> = url.query_pairs().collect();
        let params: BTreeMap<_, _> = url.query_pairs().collect();
        assert_eq!(params["filter[title]"], "Fixture  Release");
        assert_eq!(params["sorts"], "-seeders");
        assert_eq!(params["per_page"], "100");
        assert!(pairs.iter().any(|(k,v)|k=="filter[category][in][]"&&v==&c.tracker_category.to_string()),"{}",c.id);
        assert!(!r.url.contains("fixture-"));
        let mut body = payload(&c, 0., 2., 0);
        let data = body["data"]["data"].as_array_mut().unwrap();
        data.push(payload(&c, 0.5, 1., 0)["data"]["data"][0].clone());
        data.push(payload(&c, 1., 1., 0)["data"]["data"][0].clone());
        let rows = p.extract(r, &body.to_string(), &ctx).unwrap();
        assert_eq!(rows.len(), 1, "{}", c.id);
        assert_eq!(rows[0].attributes["downloadvolumefactor"], "0");
        assert!(p.extract(r, "{\"data\":[]}", &ctx).is_err());
        assert!(p.extract(r, "{}", &ctx).is_err());
        assert!(p
            .extract(r, "{\"data\":{\"data\":[]}}", &ctx)
            .unwrap()
            .is_empty());
    }
}
#[test]
fn admission_and_complete_authentication_schema_are_pinned() {
    let cases = cases();
    assert_eq!(cases.len(), 29);
    assert_eq!(
        builtin::NEXUS_API_IDS,
        cases.iter().map(|c| c.id.as_str()).collect::<Vec<_>>()
    );
    for c in &cases {
        rackett::builtin_runtime::api_auth::validate_definition(&root(c)).unwrap();
        assert!(builtin::catalog()
            .iter()
            .any(|r| r["id"] == c.id && r["ready"] == true));
    }
    let raw = builtin::SOURCES
        .iter()
        .find(|(id, _)| *id == "13city-api")
        .unwrap()
        .1;
    for (from, to) in [
        ("method: get", "method: post"),
        ("per_page: 1\n", "per_page: 2\n"),
        ("per_page: 1\n", "per_page: 1\n    per_page: 1\n"),
        ("Unauthenticated", "Other"),
        ("The API token was not accepted", "Other"),
        (
            "name: passkey\n    type: text",
            "name: passkey\n    type: checkbox",
        ),
        ("selector: \"data.data", "selector: \"other.data"),
    ] {
        assert!(raw.contains(from), "{from}");
        let n = cardigann::parse(&raw.replace(from, to), cardigann::Limits::default()).unwrap();
        assert!(
            rackett::builtin_runtime::api_auth::validate_definition(&n).is_err(),
            "{from}"
        );
    }
}

#[tokio::test]
async fn every_nexusphp_api_preset_passes_authenticated_engine_lifecycle() {
    use futures_util::{stream, StreamExt};
    stream::iter(cases())
        .for_each_concurrent(8, |case| async move {
            let id = case.id.clone();
            tokio::time::timeout(std::time::Duration::from_secs(180), lifecycle(case))
                .await
                .unwrap_or_else(|_| panic!("{id}: lifecycle timed out"));
        })
        .await;
}
async fn lifecycle(case: Case) {
    use axum::{
        extract::State,
        http::{HeaderMap, StatusCode, Uri},
        response::{IntoResponse, Response},
        routing::get,
        Router,
    };
    use rackett::{
        engine::{Download, Engine},
        models::Indexer,
    };
    use std::sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Mutex,
    };
    const KEY: &str = "fixture-api-key";
    const TORRENT: &[u8] = b"d4:infod4:name7:fixture6:lengthi42eee";
    struct Fixture {
        body: Value,
        mode: AtomicU8,
        key: Mutex<String>,
        hits: Mutex<Vec<(String, String, String)>>,
    }
    impl Fixture {
        fn take(&self) -> Vec<(String, String, String)> {
            std::mem::take(&mut *self.hits.lock().unwrap())
        }
    }
    async fn respond(State(f): State<Arc<Fixture>>, uri: Uri, h: HeaderMap) -> Response {
        let auth = h
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let cookie = h.get("cookie").and_then(|v| v.to_str().ok()).unwrap_or("");
        f.hits
            .lock()
            .unwrap()
            .push((uri.to_string(), auth.into(), cookie.into()));
        if auth != format!("Bearer {}", f.key.lock().unwrap()) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let mode = f.mode.load(Ordering::SeqCst);
        if uri.path() == "/api/v1/torrents" && uri.query() == Some("per_page=1") {
            assert!(cookie.is_empty(), "preflight inherited stale cookies");
            let mut response = match mode {
                2 => StatusCode::UNAUTHORIZED.into_response(),
                3 => "<a href='login.php?returnto=torrents'>fixture-private-secret</a>"
                    .into_response(),
                4 => "Unauthenticated fixture-private-secret".into_response(),
                5 => axum::Json(json!({"message":"fixture-private-secret"})).into_response(),
                6 => StatusCode::NO_CONTENT.into_response(),
                7 => (StatusCode::FOUND, [("location", "/api/v1/torrents")], "").into_response(),
                _ => axum::Json(json!({"data":{"data":[]}})).into_response(),
            };
            response.headers_mut().append(
                "set-cookie",
                "session=fixture-session; Path=/".parse().unwrap(),
            );
            response.headers_mut().append(
                "set-cookie",
                "api-only=fixture-scoped; Path=/api".parse().unwrap(),
            );
            return response;
        }
        if uri.path() == "/download.php" {
            assert!(uri
                .query()
                .unwrap()
                .contains("passkey=fixture-download-secret"));
            assert_eq!(cookie, "session=fixture-session");
            return TORRENT.into_response();
        }
        assert_eq!(uri.path(), "/api/v1/torrents");
        assert_eq!(cookie, "api-only=fixture-scoped; session=fixture-session");
        match mode {
            1 => axum::Json(json!({"data":{"data":[]}})).into_response(),
            8 => (StatusCode::SERVICE_UNAVAILABLE, "fixture-private-secret").into_response(),
            9 => ([("cf-mitigated", "challenge")], axum::Json(f.body.clone())).into_response(),
            10 => {
                "<a href='login.php?returnto=torrents'>fixture-private-secret</a>".into_response()
            }
            11 => axum::Json(json!({"message":"fixture-private-secret"})).into_response(),
            12 => (
                [("content-type", "application/json; charset=iso-8859-1")],
                f.body.to_string(),
            )
                .into_response(),
            _ => axum::Json(f.body.clone()).into_response(),
        }
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let fixture = Arc::new(Fixture {
        body: payload(&case, 1.0, 1.0, 0),
        mode: AtomicU8::new(0),
        key: Mutex::new(KEY.into()),
        hits: Default::default(),
    });
    let app = Router::new()
        .fallback(get(respond))
        .with_state(fixture.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let engine = Arc::new(Engine::new(path.clone()).await.unwrap());
    for passkey in [json!(null), json!(""), json!("x y"), json!("é")] {
        let bad = Indexer {
            kind: "cardigann".into(),
            name: case.id.clone(),
            url: base.clone(),
            definition: Some(
                json!({"builtin_id":case.id,"settings":{"apikey":KEY,"passkey":passkey}}),
            ),
            ..Default::default()
        };
        assert!(engine.put_indexer(bad, false).await.is_err(), "{}", case.id);
        assert!(fixture.take().is_empty());
    }
    let i = engine
        .put_indexer(
            Indexer {
                kind: "cardigann".into(),
                name: case.id.clone(),
                url: base.clone(),
                request_delay_ms: 0,
                definition: Some(json!({"builtin_id":case.id,"settings":{"apikey":KEY,"passkey":"fixture-download-secret"}})),
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap_or_else(|e| panic!("{} create: {e}", case.id));
    assert!(fixture.take().is_empty());
    assert!(!builtin::public_definition(&i)
        .unwrap()
        .to_string()
        .contains(KEY));
    assert!(!builtin::public_definition(&i)
        .unwrap()
        .to_string()
        .contains("fixture-download-secret"));
    let q = SearchQuery {
        params: BTreeMap::from([
            ("q".into(), "Fixture.Release".into()),
            ("cat".into(), case.category.to_string()),
        ]),
    };
    let first = engine.search(vec![i.id.clone()], q.clone()).await;
    assert!(first.errors.is_empty(), "{}: {:?}", case.id, first.errors);
    assert_eq!(first.results.len(), 1, "{}", case.id);
    assert_eq!(first.results[0].title, "Fixture Release");
    let hits = fixture.take();
    assert_eq!(hits.len(), 2, "{}", case.id);
    assert_eq!(hits[0].0, "/api/v1/torrents?per_page=1");
    assert!(hits
        .iter()
        .all(|(_, auth, _)| auth == &format!("Bearer {KEY}")));
    let url = url::Url::parse(&format!("{base}{}", &hits[1].0[1..])).unwrap();
    let params: BTreeMap<_, _> = url.query_pairs().collect();
    assert_eq!(params["filter[title]"], "Fixture Release", "{}", case.id);
    assert!(!url.as_str().contains(KEY));
    assert_eq!(engine.search(vec![i.id.clone()], q.clone()).await.cached, 1);
    assert!(fixture.take().is_empty());
    let release = &first.results[0];
    let mut presented = first.clone();
    rackett::protocol::present_results(
        &mut presented,
        "management-placeholder",
        "http://127.0.0.1:9117",
    );
    let public = serde_json::to_string(&presented).unwrap();
    for secret in [KEY, "fixture-session", "fixture-download-secret"] {
        assert!(!public.contains(secret), "{}", case.id);
    }
    assert!(presented.results[0]
        .link
        .starts_with("http://127.0.0.1:9117/dl/"));
    assert_eq!(
        engine
            .download(&i.id, release.source_epoch, &release.link)
            .await
            .unwrap(),
        Download::Torrent {
            bytes: TORRENT.to_vec(),
            content_type: "application/x-bittorrent".into()
        }
    );
    assert_eq!(fixture.take().len(), 2);
    assert!(engine
        .download(
            &i.id,
            release.source_epoch,
            "https://foreign.invalid/download"
        )
        .await
        .is_err());
    assert!(fixture.take().is_empty());
    engine
        .patch_indexer(&i.id, json!({"name":"Renamed"}))
        .await
        .unwrap();
    assert!(engine
        .download(&i.id, release.source_epoch, &release.link)
        .await
        .is_err());
    drop(engine);
    let engine = Arc::new(Engine::new(path).await.unwrap());
    let restarted = engine.search(vec![i.id.clone()], q.clone()).await;
    assert!(
        restarted.errors.is_empty(),
        "{} restart: {:?}",
        case.id,
        restarted.errors
    );
    assert_eq!(restarted.results.len(), 1);
    assert_eq!(fixture.take().len(), 2);
    for mode in 1..=12 {
        fixture.mode.store(mode, Ordering::SeqCst);
        engine.clear_cache().await;
        let r = engine.search(vec![i.id.clone()], q.clone()).await;
        assert!(r.results.is_empty(), "{} mode {mode}", case.id);
        assert_eq!(
            r.errors.is_empty(),
            mode == 1,
            "{} mode {mode}: {:?}",
            case.id,
            r.errors
        );
        let output = serde_json::to_string(&r).unwrap();
        for secret in [KEY, "fixture-private-secret", "fixture-download-secret"] {
            assert!(!output.contains(secret));
        }
        let hits = fixture.take();
        assert_eq!(
            hits.len(),
            if (2..=7).contains(&mode) { 1 } else { 2 },
            "{} mode {mode}",
            case.id
        );
    }
    fixture.mode.store(0, Ordering::SeqCst);
    let recovered = engine.search(vec![i.id.clone()], q.clone()).await;
    assert!(recovered.errors.is_empty());
    assert_eq!(recovered.results.len(), 1);
    assert_eq!(fixture.take().len(), 2);
    // API-key edits retire the previous epoch and immediately use the replacement.
    engine.patch_indexer(&i.id,json!({"definition":{"builtin_id":case.id,"settings":{"apikey":"fixture-rotated-key"}}})).await.unwrap();
    *fixture.key.lock().unwrap() = "fixture-rotated-key".into();
    assert!(engine
        .download(
            &i.id,
            recovered.results[0].source_epoch,
            &recovered.results[0].link
        )
        .await
        .is_err());
    assert!(fixture.take().is_empty());
    let rotated = engine.search(vec![i.id.clone()], q).await;
    assert!(rotated.errors.is_empty(), "{} rotate", case.id);
    assert_eq!(rotated.results.len(), 1);
    assert!(fixture
        .take()
        .iter()
        .all(|(_, a, _)| a == "Bearer fixture-rotated-key"));
    server.abort();
}
