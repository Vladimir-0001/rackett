//! Synthetic responses from the unchanged pinned UNIT3D definitions, never live
//! account evidence. Every admitted preset exercises its own fields and inputs.
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
    category_name: String,
    offset: String,
    filename_setting: bool,
    default_title: String,
}
fn cases() -> Vec<Case> {
    serde_json::from_str(include_str!("fixtures/unit3d-api/cases.json")).unwrap()
}
fn root(case: &Case) -> Node {
    let raw = builtin::SOURCES
        .iter()
        .find(|(id, _)| *id == case.id)
        .unwrap()
        .1;
    assert_eq!(
        format!("{:x}", Sha256::digest(raw.as_bytes())),
        case.sha256,
        "{}",
        case.id
    );
    cardigann::parse(raw, cardigann::Limits::default()).unwrap()
}
/// hawke-uno's API sends `freeleech` and `double_upload` as 0/1 (its `case` blocks).
fn numeric_promotions(case: &Case) -> bool {
    case.id == "hawke-uno"
}
fn payload(case: &Case, base: &str, featured: bool) -> Value {
    let mut body = json!({"data":[{"id":"42","type":"torrent","attributes":{
 "category_id":case.tracker_category,"category":case.category_name,"name":"Fixture Release","num_file":1,"files":[{"name":"Fixture Filename"}],
 "details_link":format!("{base}torrents/42"),"download_link":format!("{base}torrent/download/42?passkey=fixture-download-secret"),
 "meta":{"poster":"https://via.placeholder.com/90x135","genres":"Science Fiction & Fantasy"},"poster":"https://via.placeholder.com/90x135",
 "imdb_id":"1234567","tmdb_id":42,"tvdb_id":43,"internal":false,"featured":featured,"audio_pt":false,"resolution":"1080p","type":"WEB-DL",
 "seeders":11,"leechers":3,"times_completed":17,"created_at":"2026-10-28T10:00:00.123456Z","size":1073741824,"freeleech":"0%","double_upload":if case.id=="jme-reunit3d-api"{json!(0)}else{json!(false)},
 "info_hash":"0123456789abcdef0123456789abcdef01234567"}}]});
    if numeric_promotions(case) {
        body["data"][0]["attributes"]["freeleech"] = json!(0);
        body["data"][0]["attributes"]["double_upload"] = json!(0);
    }
    body
}
#[test]
fn all_pinned_unit3d_rows_use_real_iso_dates_and_boolean_scalars() {
    let mut failures = vec![];
    for case in cases() {
        let p = Plan::compile(&root(&case)).unwrap();
        assert!(
            p.blockers().iter().all(|b| b.path == "$.login"),
            "{}",
            case.id
        );
        let settings = BTreeMap::from([("apikey".into(), json!("fixture-api-key"))]);
        let q = SearchQuery {
            params: BTreeMap::from([("q".into(), "Fixture".into())]),
        };
        let ctx = p.context(&settings, &q, "2026-10-28T12:00:00Z".parse().unwrap());
        let req = p.build_requests(&ctx).unwrap().remove(0);
        match p.extract(
            &req,
            &payload(&case, "https://fixture.invalid/", false).to_string(),
            &ctx,
        ) {
            Err(e) => failures.push(format!("{}: {e}", case.id)),
            Ok(rows) => {
                if rows.len() != 1 {
                    failures.push(format!("{}: {} rows", case.id, rows.len()));
                    continue;
                }
                let r = &rows[0];
                let expected_title = &case.default_title;
                let expected_date = match case.offset.as_str() {
                    " -03:00" => "2026-10-28T13:00:00Z",
                    " +01:00" => "2026-10-28T09:00:00Z",
                    " +02:00" => "2026-10-28T08:00:00Z",
                    _ => "2026-10-28T10:00:00Z",
                };
                for (ok, name) in [
                    (r.title == *expected_title, "title"),
                    (r.size == Some(1073741824), "size"),
                    (r.seeders == Some(11), "seeders"),
                    (r.peers == Some(14), "peers"),
                    (r.categories.contains(&case.category), "category"),
                    (r.published.as_deref() == Some(expected_date), "date"),
                    (
                        r.attributes.get("downloadvolumefactor").map(String::as_str) == Some("1"),
                        "download factor",
                    ),
                    (
                        r.attributes.get("uploadvolumefactor").map(String::as_str) == Some("1"),
                        "upload factor",
                    ),
                ] {
                    if !ok {
                        failures.push(format!("{}: wrong {name}: {:?}", case.id, r));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[tokio::test]
async fn every_unit3d_preset_passes_authenticated_engine_lifecycle() {
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
        if uri.path() == "/api/torrents" {
            assert!(cookie.is_empty(), "preflight inherited stale cookies");
            let mut response = match mode {
                2 => StatusCode::UNAUTHORIZED.into_response(),
                3 => "<a href='/login'>fixture-private-secret</a>".into_response(),
                4 => "Account is Banned fixture-private-secret".into_response(),
                5 => axum::Json(json!({"message":"fixture-private-secret"})).into_response(),
                6 => StatusCode::NO_CONTENT.into_response(),
                7 => (StatusCode::FOUND, [("location", "/api/torrents")], "").into_response(),
                _ => axum::Json(json!({"data":[]})).into_response(),
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
        if uri.path().starts_with("/torrent/download/") {
            assert_eq!(cookie, "session=fixture-session");
            return TORRENT.into_response();
        }
        assert_eq!(uri.path(), "/api/torrents/filter");
        assert_eq!(cookie, "api-only=fixture-scoped; session=fixture-session");
        match mode {
            1 => axum::Json(json!({"data":[]})).into_response(),
            8 => (StatusCode::SERVICE_UNAVAILABLE, "fixture-private-secret").into_response(),
            9 => ([("cf-mitigated", "challenge")], axum::Json(f.body.clone())).into_response(),
            10 => "<a href='/login'>fixture-private-secret</a>".into_response(),
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
        body: payload(&case, &base, false),
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
    let i = engine
        .put_indexer(
            Indexer {
                kind: "cardigann".into(),
                name: case.id.clone(),
                url: base.clone(),
                request_delay_ms: 0,
                definition: Some(json!({"builtin_id":case.id,"settings":{"apikey":KEY}})),
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
    let q = SearchQuery {
        params: BTreeMap::from([
            ("q".into(), "Fixture.Release".into()),
            ("cat".into(), case.category.to_string()),
        ]),
    };
    let first = engine.search(vec![i.id.clone()], q.clone()).await;
    assert!(first.errors.is_empty(), "{}: {:?}", case.id, first.errors);
    assert_eq!(first.results.len(), 1, "{}", case.id);
    assert_eq!(first.results[0].title, case.default_title);
    let hits = fixture.take();
    assert_eq!(hits.len(), 2, "{}", case.id);
    assert_eq!(hits[0].0, "/api/torrents");
    assert!(hits
        .iter()
        .all(|(_, auth, _)| auth == &format!("Bearer {KEY}")));
    let url = url::Url::parse(&format!("{base}{}", &hits[1].0[1..])).unwrap();
    let params: BTreeMap<_, _> = url.query_pairs().collect();
    assert_eq!(params["name"], "Fixture Release", "{}", case.id);
    assert!(!url.as_str().contains(KEY));
    assert_eq!(engine.search(vec![i.id.clone()], q.clone()).await.cached, 1);
    assert!(fixture.take().is_empty());
    let release = &first.results[0];
    let mut presented = first.clone();
    let links = engine
        .links(
            &presented.results,
            "management-placeholder",
            "http://127.0.0.1:9117",
        )
        .await;
    rackett::protocol::present_results(&mut presented, &links);
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

#[test]
fn family_promotion_cases_do_not_treat_false_as_truthy_and_filename_is_optional() {
    for case in cases() {
        let p = Plan::compile(&root(&case)).unwrap();
        let ctx = p.context(
            &BTreeMap::new(),
            &SearchQuery::default(),
            chrono::Utc::now(),
        );
        let req = p.build_requests(&ctx).unwrap().remove(0);
        let has_featured = root(&case)
            .get("search")
            .unwrap()
            .get("fields")
            .unwrap()
            .get("_featured")
            .is_some();
        let promotions = if numeric_promotions(&case) {
            vec![
                (json!(0), json!(0), false, "1", "1"),
                (json!(1), json!(1), false, "0", "2"),
                (json!(0), json!(0), true, "1", "1"),
            ]
        } else {
            vec![
                (json!("0%"), json!(false), false, "1", "1"),
                (json!("50%"), json!(false), false, "0.5", "1"),
                (json!("100%"), json!(true), false, "0", "2"),
                (
                    json!("0%"),
                    json!(false),
                    true,
                    if has_featured { "0" } else { "1" },
                    if has_featured { "2" } else { "1" },
                ),
            ]
        };
        for (free, double, featured, download, upload) in promotions {
            let mut body = payload(&case, "https://fixture.invalid/", featured);
            body["data"][0]["attributes"]["freeleech"] = free;
            body["data"][0]["attributes"]["double_upload"] =
                if case.id == "jme-reunit3d-api" && double.is_boolean() {
                    json!(u8::from(double == json!(true)))
                } else {
                    double
                };
            let rows = p
                .extract(&req, &body.to_string(), &ctx)
                .unwrap_or_else(|e| panic!("{}: {e}", case.id));
            let (download, upload) = if case.id == "monikadesign-api" {
                ("1", "1")
            } else {
                (download, upload)
            };
            assert_eq!(
                rows[0].attributes["downloadvolumefactor"], download,
                "{}",
                case.id
            );
            assert_eq!(
                rows[0].attributes["uploadvolumefactor"], upload,
                "{}",
                case.id
            );
        }
        if case.filename_setting {
            let settings =
                BTreeMap::from([("single_file_release_use_filename".into(), json!(false))]);
            let ctx = p.context(&settings, &SearchQuery::default(), chrono::Utc::now());
            let body = payload(&case, "https://fixture.invalid/", false);
            let rows = p.extract(&req, &body.to_string(), &ctx).unwrap();
            assert!(!rows[0].title.contains("Filename"), "{}", case.id);
            let mut body = body;
            body["data"][0]["attributes"]
                .as_object_mut()
                .unwrap()
                .remove("files");
            let rows = p.extract(&req, &body.to_string(), &ctx).unwrap();
            assert!(!rows[0].title.contains("Filename"));
        }
    }
}

#[test]
fn family_requests_preserve_ids_categories_freeleech_sort_and_dotted_keywords() {
    for case in cases() {
        let root = root(&case);
        let p = Plan::compile(&root).unwrap();
        let declared: Vec<_> = root
            .get("settings")
            .unwrap()
            .items()
            .unwrap()
            .iter()
            .filter_map(|s| s.string("name"))
            .collect();
        let mut settings = BTreeMap::from([
            ("apikey".into(), json!("fixture-api-key")),
            ("freeleech".into(), json!(true)),
            ("sort".into(), json!("seeders")),
            ("type".into(), json!("asc")),
        ]);
        if declared.contains(&"tmdbidonly") {
            settings.insert("tmdbidonly".into(), json!(false));
        }
        p.check_settings(&settings).unwrap();
        let q = SearchQuery {
            params: BTreeMap::from([
                ("q".into(), "Fixture..Release".into()),
                ("t".into(), "tvsearch".into()),
                ("season".into(), "1".into()),
                ("ep".into(), "2".into()),
                ("imdbid".into(), "tt1234567".into()),
                ("tmdbid".into(), "42".into()),
                ("tvdbid".into(), "43".into()),
                ("cat".into(), case.category.to_string()),
            ]),
        };
        let ctx = p.context(&settings, &q, chrono::Utc::now());
        let requests = p.build_requests(&ctx).unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.method, "GET");
        assert!(request.body.is_none());
        assert_eq!(
            request.headers,
            [("Authorization".into(), "Bearer fixture-api-key".into())]
        );
        let u = url::Url::parse(&request.url).unwrap();
        assert_eq!(u.path(), "/api/torrents/filter");
        let pairs: Vec<_> = u.query_pairs().collect();
        let params: BTreeMap<_, _> = u.query_pairs().collect();
        // ShareIsland explicitly removes the season/episode token before the
        // dot replacement; it still sends the separate season/episode inputs.
        let name = if case.id == "shareisland-api" {
            "Fixture  Release "
        } else {
            "Fixture  Release S01E02"
        };
        assert_eq!(params["name"], name, "{}", case.id);
        assert_eq!(params["sortField"], "seeders");
        assert_eq!(params["sortDirection"], "asc");
        assert_eq!(params["perPage"], "100");
        assert!(!request.url.contains("fixture-api-key"));
        if case.id == "clearjav-api" {
            assert!(!params.contains_key("categories[]"));
        } else {
            assert!(
                pairs
                    .iter()
                    .any(|(k, v)| k == "categories[]" && v == &case.tracker_category.to_string()),
                "{}",
                case.id
            );
        }
        let inputs = root.get("search").unwrap().get("inputs").unwrap();
        for (key, value) in [
            ("imdbId", "1234567"),
            ("tmdbId", "42"),
            ("tvdbId", "43"),
            ("seasonNumber", "1"),
            ("episodeNumber", "2"),
            ("free[]", "100"),
            ("free", "1"),
        ] {
            if inputs.get(key).is_some() {
                assert_eq!(
                    params.get(key).map(|s| s.as_ref()),
                    Some(value),
                    "{} {key}",
                    case.id
                );
            } else {
                assert!(!params.contains_key(key), "{} {key}", case.id);
            }
        }
        if declared.contains(&"tmdbidonly") {
            settings.insert("tmdbidonly".into(), json!(true));
            let ctx = p.context(&settings, &q, chrono::Utc::now());
            let r = p.build_requests(&ctx).unwrap().remove(0);
            let url = url::Url::parse(&r.url).unwrap();
            let params: BTreeMap<_, _> = url.query_pairs().collect();
            assert!(!params.contains_key("imdbId"));
            assert!(!params.contains_key("tvdbId"));
            assert_eq!(params["tmdbId"], "42");
        }
    }
}
#[test]
fn api_admission_matches_the_source_pinned_fixture_manifest() {
    let cases = cases();
    assert_eq!(cases.len(), 72);
    let ids: Vec<_> = cases.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(builtin::API_IDS, ids);
    for case in cases {
        rackett::builtin_runtime::api_auth::validate_definition(&root(&case)).unwrap();
        assert!(builtin::READY_IDS.contains(&case.id.as_str()));
        let catalog = builtin::catalog()
            .iter()
            .find(|v| v["id"] == case.id)
            .unwrap();
        assert_eq!(catalog["ready"], true, "{}", case.id);
    }
}
