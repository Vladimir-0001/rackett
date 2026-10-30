//! Source-derived NexusPHP fixtures. These prove interpreter/runtime behavior,
//! never live account availability. Admission is a separate explicit catalog gate.
use rackett::{builtin, cardigann, cardigann_exec::Plan, models::SearchQuery};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Clone, Deserialize)]
struct Case {
    id: String,
    sha256: String,
    tracker_category: u32,
    category: u32,
    layout: String,
    published: String,
}

/// Public layout cases: the original manifest plus independently merged batches.
const PUBLIC_MANIFESTS: [&str; 11] = [
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
];
/// Cookie layout cases: the original manifest plus independently merged batches.
const COOKIE_MANIFESTS: [&str; 5] = [
    include_str!("fixtures/cookie-layouts/cases.json"),
    include_str!("fixtures/cookie-batches/batch1.json"),
    include_str!("fixtures/cookie-batches/batch2.json"),
    include_str!("fixtures/cookie-batches/batch3.json"),
    include_str!("fixtures/cookie-batches/batch4.json"),
];
#[tokio::test]
async fn every_admitted_nexusphp_case_passes_engine_session_search_download_and_restart() {
    use futures_util::{stream, StreamExt};
    // Separate loopback origins, configs and engines: only concurrency is shared.
    stream::iter(cases())
        .for_each_concurrent(8, |case| async move {
            let id = case.id.clone();
            tokio::time::timeout(std::time::Duration::from_secs(180), lifecycle(case))
                .await
                .unwrap_or_else(|_| panic!("{id}: lifecycle deadline exceeded"));
        })
        .await;
}

async fn lifecycle(case: Case) {
    use axum::{
        extract::State,
        http::{HeaderMap, StatusCode, Uri},
        response::{Html, IntoResponse},
        routing::get,
        Router,
    };
    use rackett::{
        engine::{Download, Engine},
        models::Indexer,
    };
    use serde_json::json;
    use std::sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Mutex,
    };
    const SEED: &str = "session=fixture-seed-cookie";
    const REFRESHED: &str = "session=fixture-refreshed-cookie";
    const TORRENT: &[u8] = b"d4:infod4:name7:fixture6:lengthi42eee";
    struct Fixture {
        page: String,
        mode: AtomicU8,
        hits: Mutex<Vec<(String, String)>>,
    }
    impl Fixture {
        fn take(&self) -> Vec<(String, String)> {
            std::mem::take(&mut *self.hits.lock().unwrap())
        }
    }
    async fn respond(
        State(f): State<Arc<Fixture>>,
        uri: Uri,
        h: HeaderMap,
    ) -> axum::response::Response {
        let cookie = h.get("cookie").and_then(|h| h.to_str().ok()).unwrap_or("");
        f.hits
            .lock()
            .unwrap()
            .push((uri.to_string(), cookie.into()));
        if cookie != SEED && cookie != REFRESHED {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let mode = f.mode.load(Ordering::SeqCst);
        if mode == 2 {
            return Html("<form>Sign in</form>").into_response();
        }
        if mode == 4 {
            return (StatusCode::SERVICE_UNAVAILABLE, "fixture-private-error").into_response();
        }
        if uri.path() == "/download.php" {
            return TORRENT.into_response();
        }
        let body = if mode == 1 {
            format!(
                "{}<table class=torrents><tbody></tbody></table></body></html>",
                f.page.split("<table class=torrents>").next().unwrap()
            )
        } else {
            f.page.clone()
        };
        let mut response = Html(body).into_response();
        if cookie == SEED {
            response.headers_mut().insert(
                "set-cookie",
                "session=fixture-refreshed-cookie; Path=/".parse().unwrap(),
            );
        }
        if mode == 3 {
            response
                .headers_mut()
                .insert("cf-mitigated", "challenge".parse().unwrap());
        }
        response
    }
    let fixture = Arc::new(Fixture {
        page: page(&case, "free"),
        mode: AtomicU8::new(0),
        hits: Default::default(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new()
        .fallback(get(respond))
        .with_state(fixture.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let engine = Arc::new(Engine::new(path.clone()).await.unwrap());
    // As in tests/form_layout_runtime.rs: pinned request delays and concurrent
    // lifecycles exceed the 20 s default deadline, which is not under test here.
    engine
        .update_settings(json!({"request_timeout_seconds": 60}))
        .await
        .unwrap();
    let i = engine
        .put_indexer(
            Indexer {
                kind: "cardigann".into(),
                name: case.id.clone(),
                url: base.clone(),
                request_delay_ms: 0,
                definition: Some(
                    json!({"builtin_id":case.id,"settings":{"cookie":SEED,"freeleech":false}}),
                ),
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap_or_else(|e| panic!("{} create: {e}", case.id));
    assert!(
        fixture.take().is_empty(),
        "{}: create contacted tracker",
        case.id
    );
    let public = builtin::public_definition(&i).unwrap().to_string();
    assert!(!public.contains(SEED));
    let query = SearchQuery {
        params: BTreeMap::from([
            ("q".into(), "Fixture".into()),
            ("cat".into(), case.category.to_string()),
        ]),
    };
    let first = engine.search(vec![i.id.clone()], query.clone()).await;
    assert!(first.errors.is_empty(), "{}: {:?}", case.id, first.errors);
    assert_eq!(first.results.len(), 1, "{}", case.id);
    let release = &first.results[0];
    assert_eq!(release.title, "Fixture Release", "{}", case.id);
    assert_eq!(release.size, Some(1073741824), "{}", case.id);
    assert_eq!(release.seeders, Some(11), "{}", case.id);
    assert_eq!(
        release.published.as_deref(),
        Some(case.published.as_str()),
        "{}",
        case.id
    );
    assert!(release.categories.contains(&case.category), "{}", case.id);
    let hits = fixture.take();
    assert!(hits.len() >= 2, "{}", case.id);
    assert_eq!(hits[0], ("/index.php".into(), SEED.into()), "{}", case.id);
    assert!(hits[1..].iter().all(|(_, c)| c == REFRESHED), "{}", case.id);
    for (path, _) in hits.iter().filter(|(p, _)| !p.starts_with("/index.php")) {
        let url = url::Url::parse(&format!("{base}{}", &path[1..])).unwrap();
        assert!(
            matches!(url.path(), "/torrents.php" | "/special.php"),
            "{}",
            case.id
        );
        let pairs: BTreeMap<_, _> = url.query_pairs().collect();
        assert_eq!(
            pairs.get("search").map(|s| s.as_ref()),
            Some("Fixture"),
            "{}: {url}",
            case.id
        );
        assert!(!url.as_str().contains("fixture-seed-cookie"));
        assert!(!url.as_str().contains("fixture-refreshed-cookie"));
    }
    assert_eq!(
        engine
            .search(vec![i.id.clone()], query.clone())
            .await
            .cached,
        1,
        "{}",
        case.id
    );
    assert!(fixture.take().is_empty(), "{}", case.id);
    let mut displayed = first.clone();
    rackett::protocol::present_results(
        &mut displayed,
        "fixture-management-key",
        "http://127.0.0.1:9117",
    );
    let json = serde_json::to_string(&displayed).unwrap();
    for secret in [SEED, REFRESHED, "fixture-download-secret"] {
        assert!(!json.contains(secret), "{}", case.id);
    }
    assert!(
        displayed.results[0]
            .link
            .starts_with("http://127.0.0.1:9117/dl/"),
        "{}",
        case.id
    );
    assert_eq!(
        engine
            .download(&i.id, release.source_epoch, &release.link)
            .await
            .unwrap_or_else(|e| panic!("{} download: {e}", case.id)),
        Download::Torrent {
            bytes: TORRENT.to_vec(),
            content_type: "application/x-bittorrent".into()
        }
    );
    let hits = fixture.take();
    assert_eq!(hits.len(), 2, "{}", case.id);
    assert!(hits.iter().all(|(_, c)| c == REFRESHED), "{}", case.id);
    assert!(engine
        .download(
            &i.id,
            release.source_epoch,
            "https://foreign.invalid/download.php?id=42"
        )
        .await
        .is_err());
    assert!(fixture.take().is_empty());
    engine
        .patch_indexer(&i.id, json!({"name":format!("Renamed {}",case.id)}))
        .await
        .unwrap();
    assert!(engine
        .download(&i.id, release.source_epoch, &release.link)
        .await
        .is_err());
    drop(engine);
    let engine = Arc::new(Engine::new(path).await.unwrap());
    let restarted = engine.search(vec![i.id.clone()], query.clone()).await;
    assert!(
        restarted.errors.is_empty(),
        "{} restart: {:?}",
        case.id,
        restarted.errors
    );
    assert_eq!(restarted.results.len(), 1, "{}", case.id);
    let hits = fixture.take();
    assert!(
        hits.len() >= 2 && hits.iter().all(|(_, c)| c == REFRESHED),
        "{}",
        case.id
    );
    for mode in [1, 2, 3, 4] {
        engine.clear_cache().await;
        fixture.mode.store(mode, Ordering::SeqCst);
        let response = engine.search(vec![i.id.clone()], query.clone()).await;
        assert!(response.results.is_empty(), "{} mode {mode}", case.id);
        assert_eq!(
            response.errors.is_empty(),
            mode == 1,
            "{} mode {mode}: {:?}",
            case.id,
            response.errors
        );
        assert!(!serde_json::to_string(&response)
            .unwrap()
            .contains("fixture-private-error"));
        let hits = fixture.take();
        assert!(!hits.is_empty(), "{} mode {mode}", case.id);
        assert!(
            hits.iter().all(|(_, c)| c == REFRESHED),
            "{} mode {mode}",
            case.id
        );
    }
    fixture.mode.store(0, Ordering::SeqCst);
    let recovered = engine.search(vec![i.id.clone()], query).await;
    assert!(
        recovered.errors.is_empty(),
        "{} recover: {:?}",
        case.id,
        recovered.errors
    );
    assert_eq!(recovered.results.len(), 1, "{}", case.id);
    assert!(!fixture.take().is_empty(), "{}: cached failure", case.id);
    server.abort();
}
fn cases() -> Vec<Case> {
    serde_json::from_str(include_str!("fixtures/cookie-nexusphp/cases.json")).unwrap()
}
#[test]
fn catalog_admission_is_exactly_the_baseline_and_the_source_pinned_family() {
    use std::collections::BTreeSet;
    let family: BTreeSet<_> = cases().into_iter().map(|c| c.id).collect();
    assert_eq!(family.len(), 57);
    let mut cookie = family.clone();
    cookie.insert("musopia".into());
    let layouts: Vec<serde_json::Value> = COOKIE_MANIFESTS
        .iter()
        .flat_map(|m| serde_json::from_str::<Vec<serde_json::Value>>(m).unwrap())
        .collect();
    cookie.extend(layouts.iter().map(|v| v["id"].as_str().unwrap().to_owned()));
    assert_eq!(
        builtin::COOKIE_IDS
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>(),
        cookie
    );
    let mut ready = family;
    ready.extend(layouts.iter().map(|v| v["id"].as_str().unwrap().to_owned()));
    let forms: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/form-layouts/cases.json")).unwrap();
    ready.extend(forms.iter().map(|v| v["id"].as_str().unwrap().to_owned()));
    let posts: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/post-layouts/cases.json")).unwrap();
    ready.extend(posts.iter().map(|v| v["id"].as_str().unwrap().to_owned()));
    for batch in [
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
    ] {
        let batch: Vec<serde_json::Value> = serde_json::from_str(batch).unwrap();
        ready.extend(batch.iter().map(|v| v["id"].as_str().unwrap().to_owned()));
    }
    let public: Vec<serde_json::Value> = PUBLIC_MANIFESTS
        .iter()
        .flat_map(|m| serde_json::from_str::<Vec<serde_json::Value>>(m).unwrap())
        .collect();
    ready.extend(public.iter().map(|v| v["id"].as_str().unwrap().to_owned()));
    let api: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/unit3d-api/cases.json")).unwrap();
    ready.extend(api.iter().map(|v| v["id"].as_str().unwrap().to_owned()));
    let nexus_api: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/nexusphp-api/cases.json")).unwrap();
    ready.extend(
        nexus_api
            .iter()
            .map(|v| v["id"].as_str().unwrap().to_owned()),
    );
    for id in [
        "1337x",
        "acgrip",
        "btetree",
        "eztv",
        "milkie",
        "musopia",
        "tokyotosho",
        "uindex",
        "zamundalife",
    ] {
        ready.insert(id.into());
    }
    assert_eq!(
        builtin::READY_IDS
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<_>>(),
        ready
    );
    for row in builtin::catalog() {
        assert_eq!(
            row["ready"],
            ready.contains(row["id"].as_str().unwrap()),
            "{}",
            row["id"]
        );
    }
}

#[test]
fn every_family_request_preserves_category_input_dialect_and_changed_settings() {
    use serde_json::json;
    for case in cases() {
        let p = Plan::compile(&root(&case)).unwrap();
        let q = SearchQuery {
            params: BTreeMap::from([
                ("q".into(), "Fixture & More".into()),
                ("cat".into(), case.category.to_string()),
            ]),
        };
        let settings = BTreeMap::from([
            ("freeleech".into(), json!(true)),
            ("sort".into(), json!("7")),
            ("type".into(), json!("asc")),
        ]);
        p.check_settings(&settings).unwrap();
        let ctx = p.context(&settings, &q, chrono::Utc::now());
        let requests = p.build_requests(&ctx).unwrap();
        let mut found_category = false;
        for request in requests {
            assert_eq!(request.method, "GET", "{}", case.id);
            assert!(
                request.body.is_none() && request.headers.is_empty(),
                "{}",
                case.id
            );
            let url = url::Url::parse(&request.url).unwrap();
            let pairs: Vec<_> = url.query_pairs().collect();
            for (k, v) in [
                ("search", "Fixture & More"),
                ("spstate", if case.id == "tjupt" { "9" } else { "2" }),
                ("sort", "7"),
                ("type", "asc"),
            ] {
                assert!(
                    pairs.iter().any(|(key, value)| key == k && value == v),
                    "{}: missing {k}={v}",
                    case.id
                );
            }
            let (key, value) = match case.id.as_str() {
                "opencd" => (format!("source{}", case.tracker_category), "1".into()),
                "ourbits" => ("cat[]".into(), case.tracker_category.to_string()),
                _ => (format!("cat{}", case.tracker_category), "1".into()),
            };
            found_category |= pairs.iter().any(|(k, v)| *k == key && *v == value);
        }
        assert!(found_category, "{}: missing category", case.id);
    }
}

#[test]
fn family_added_dates_title_fallback_and_staff_columns_match_pinned_variants() {
    for case in cases() {
        let r = root(&case);
        let p = Plan::compile(&r).unwrap();
        let q = SearchQuery {
            params: BTreeMap::from([("q".into(), "Fixture".into())]),
        };
        let ctx = p.context(
            &BTreeMap::new(),
            &q,
            "2026-10-28T12:00:00Z".parse().unwrap(),
        );
        let request = p.build_requests(&ctx).unwrap().remove(0);
        let mut html = page(&case, "ordinary").replace(
            "<span title='2026-10-28 10:00:00'>2 hours</span>",
            "2026-10-28<br>10:00:00",
        );
        if r.get("search")
            .unwrap()
            .get("fields")
            .unwrap()
            .get("title")
            .unwrap()
            .string("optional")
            == Some("true")
        {
            let missing_title = html.replace("title=\"Fixture Release\"", "");
            if case.id == "hdhome" {
                // Its title has a fallback, but its required details selector also
                // needs [title]. The pinned definition rejects this whole row.
                assert_eq!(
                    p.extract(&request, &missing_title, &ctx).unwrap_err().path,
                    "$.search.fields.details"
                );
            } else {
                html = missing_title;
            }
        }
        if case.id == "keepfriends" {
            // A staff edit column shifts all date/size/peer columns by one.
            html = html.replace(
                "<td class=\"rowfollow\">0</td>",
                "<td><a href='edit.php?id=42'>Edit</a></td><td class='rowfollow'>0</td>",
            );
        }
        let rows = p
            .extract(&request, &html, &ctx)
            .unwrap_or_else(|e| panic!("{}: {e}", case.id));
        assert_eq!(rows.len(), 1, "{}", case.id);
        assert_eq!(rows[0].title, "Fixture Release", "{}", case.id);
        assert_eq!(rows[0].size, Some(1073741824), "{}", case.id);
        assert_eq!(rows[0].seeders, Some(11), "{}", case.id);
        assert_eq!(
            rows[0].published.as_deref(),
            Some(case.published.as_str()),
            "{}",
            case.id
        );
    }
}
fn root(case: &Case) -> cardigann::Node {
    let raw = builtin::SOURCES
        .iter()
        .find(|(id, _)| *id == case.id)
        .unwrap()
        .1;
    assert_eq!(
        format!("{:x}", Sha256::digest(raw.as_bytes())),
        case.sha256,
        "{} source changed",
        case.id
    );
    cardigann::parse(raw, cardigann::Limits::default()).unwrap()
}
fn page(case: &Case, promo: &str) -> String {
    let (image, span) = match promo {
        "free" => ("pro_free2up", "torrent-pro-twoupfree"),
        "half" => ("pro_50pctdown", "torrent-pro-halfdown"),
        _ => ("ordinary", "ordinary"),
    };
    let date = if case.id == "pignetwork" {
        "2026-10-28<br>10:00:00"
    } else {
        "<span title='2026-10-28 10:00:00'>2 hours</span>"
    };
    let row = include_str!("fixtures/cookie-nexusphp/row.html")
        .replace("__CATEGORY__", &case.tracker_category.to_string())
        .replace(
            "__BEFORE_TITLE__",
            if case.layout == "music" {
                "<td class=rowfollow>Format</td>"
            } else {
                ""
            },
        )
        .replace(
            "__BEFORE_DATE__",
            if case.layout == "music" {
                "<td class=rowfollow>Codec</td>"
            } else {
                ""
            },
        )
        .replace(
            "__TRAILING__",
            if case.layout == "trailing" {
                "<td class=rowfollow>Uploader</td>"
            } else {
                ""
            },
        )
        .replace("__DATE__", date)
        .replace("__PROMO__", image)
        .replace("__PROMO_SPAN__", span);
    format!(
        "<html><body><select name=category><option>All</option></select>\
        <a href='logout.php'>Logout</a><a href='logout.php?csrf_token=fixture'>Logout</a>\
        <a href='logout.php?key=fixture'>Logout</a><a href='logout.php?token=fixture'>Logout</a>\
        <a href='logout.php?auth=fixture'>Logout</a>\
        <a onclick=\"document.getElementById('logout').submit();\">Logout</a>\
        <a href='usercp.php'>Profile</a><a href='/usercp.php'>Profile</a>\
        <table class=torrents><tbody><tr><th>Heading</th></tr>{row}</tbody></table></body></html>"
    )
}

#[test]
fn pinned_nexusphp_rows_have_expected_fields_categories_and_promotion_factors() {
    let mut failures = Vec::new();
    for case in cases() {
        let plan = Plan::compile(&root(&case)).unwrap();
        assert!(
            plan.blockers().iter().all(|b| b.path == "$.login"),
            "{}",
            case.id
        );
        let query = SearchQuery {
            params: BTreeMap::from([
                ("q".into(), "Fixture".into()),
                ("cat".into(), case.category.to_string()),
            ]),
        };
        let context = plan.context(
            &BTreeMap::new(),
            &query,
            "2026-10-28T12:00:00Z".parse().unwrap(),
        );
        let requests = plan.build_requests(&context).unwrap();
        assert!(!requests.is_empty(), "{}", case.id);
        for (promo, factor, upload) in [
            ("ordinary", "1", "1"),
            ("free", "0", "2"),
            ("half", "0.5", "1"),
        ] {
            for request in &requests {
                let rows = match plan.extract(request, &page(&case, promo), &context) {
                    Ok(rows) => rows,
                    Err(error) => {
                        failures.push(format!("{} {promo}: {error}", case.id));
                        continue;
                    }
                };
                if rows.len() != 1 {
                    failures.push(format!("{}: expected one row, got {}", case.id, rows.len()));
                    continue;
                }
                let row = &rows[0];
                let checks = [
                    (row.title == "Fixture Release", "title"),
                    (row.size == Some(1073741824), "size"),
                    (row.seeders == Some(11), "seeders"),
                    (row.peers == Some(14), "peers"),
                    (row.categories.contains(&case.category), "category"),
                    (
                        row.published.as_deref() == Some(case.published.as_str()),
                        "date",
                    ),
                    (
                        row.attributes
                            .get("downloadvolumefactor")
                            .map(String::as_str)
                            == Some(factor),
                        "download factor",
                    ),
                    (
                        row.attributes.get("uploadvolumefactor").map(String::as_str)
                            == Some(upload),
                        "upload factor",
                    ),
                    (row.link.contains("download.php?id=42"), "download"),
                ];
                for (ok, name) in checks {
                    if !ok {
                        failures.push(format!("{} {promo}: wrong {name}: {row:?}", case.id));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
