//! Source-pinned public HTML/JSON fixtures. Synthetic pages prove bounded runtime
//! behavior; they do not prove live availability or classify arbitrary challenges.
use rackett::{
    builtin, cardigann,
    cardigann_exec::Plan,
    models::{Indexer, SearchQuery},
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const HASH: &str = "0123456789abcdef0123456789abcdef01234567";
/// A `download.selectors` source: its release link is a detail page whose selector
/// leads to `resolved`, a same-origin torrent path or an expected magnet URI.
#[derive(Clone, Deserialize)]
struct Detail {
    html: String,
    resolved: String,
}
#[derive(Clone, Deserialize)]
struct Case {
    id: String,
    #[serde(default)]
    detail: Option<Detail>,
    sha256: String,
    category: u32,
    request_count: usize,
    search_path: String,
    #[serde(default)]
    search_paths: Vec<String>,
    #[serde(default)]
    optional_size: bool,
    query_pair: [String; 2],
    download_path: String,
    details_path: String,
    seeders: u64,
    peers: u64,
    /// Expected `published`. A `date: now` definition yields the harness clock,
    /// `2026-10-29T12:00:00Z`, in the offline extraction test.
    published: String,
    /// Expected size in bytes when the definition fixes one (default 1 GiB).
    #[serde(default)]
    expected_size: Option<u64>,
    /// The definition's size field is a literal (`text: 512 MB`), so page text cannot
    /// make it malformed; the extraction must still yield `expected_size`.
    #[serde(default)]
    fixed_size: bool,
    /// Requests built for a keywordless query when it differs from `request_count`
    /// (e.g. category-expanded paths).
    #[serde(default)]
    keywordless_request_count: Option<usize>,
    /// Source settings, e.g. an API key a `login: get` probe and searches send.
    #[serde(default)]
    settings: serde_json::Map<String, serde_json::Value>,
    /// The exact `login: get` probe request (path and query). It is answered with an
    /// empty capabilities document and not counted among search requests.
    #[serde(default)]
    probe: Option<String>,
    /// Rendered search headers every request (probe, search, download) must carry,
    /// e.g. `{"authorization": "Bearer fixture-source-key"}`. Names are lower case.
    #[serde(default)]
    expected_headers: BTreeMap<String, String>,
    delay_ms: u64,
    title: String,
    json: bool,
    container: Option<String>,
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
fn cases() -> Vec<Case> {
    PUBLIC_MANIFESTS
        .iter()
        .flat_map(|m| serde_json::from_str::<Vec<Case>>(m).unwrap())
        .collect()
}
fn root(c: &Case) -> cardigann::Node {
    let raw = builtin::SOURCES
        .iter()
        .find(|(id, _)| *id == c.id)
        .unwrap()
        .1;
    assert_eq!(format!("{:x}", Sha256::digest(raw)), c.sha256, "{}", c.id);
    cardigann::parse(raw, cardigann::Limits::default()).unwrap()
}
fn page(c: &Case) -> String {
    std::fs::read_to_string(format!(
        "tests/fixtures/public-layouts/{}.{}",
        c.id,
        if c.json { "json" } else { "html" }
    ))
    .unwrap()
}
fn empty(c: &Case, body: &str) -> String {
    if c.id == "dmhy" {
        return include_str!("fixtures/public-layouts/dmhy-empty.html").into();
    }
    if c.id == "therarbg" {
        return r#"{"results":[]}"#.into();
    }
    if c.id == "internetarchive" {
        return r#"{"response":{"numFound":0,"docs":[]}}"#.into();
    }
    let (before, rest) = body.split_once("<!-- rows:start -->").unwrap();
    let (_, after) = rest.split_once("<!-- rows:end -->").unwrap();
    format!("{before}{after}")
}
fn query(c: &Case) -> SearchQuery {
    SearchQuery {
        params: BTreeMap::from([
            ("q".into(), "Fixture".into()),
            ("cat".into(), c.category.to_string()),
        ]),
    }
}
fn stock(c: &Case, base: &str) -> Indexer {
    Indexer {
        kind: "cardigann".into(),
        name: c.id.clone(),
        url: base.into(),
        request_delay_ms: 0,
        definition: Some(json!({"builtin_id":c.id,"settings":c.settings})),
        ..Default::default()
    }
}
fn request_matches(c: &Case, url: &url::Url) {
    if c.search_paths.is_empty() {
        assert_eq!(url.path(), c.search_path, "{}", c.id);
    } else {
        assert!(
            c.search_paths.iter().any(|p| p == url.path()),
            "{}: unexpected page",
            c.id
        );
    }
    let [key, value] = &c.query_pair;
    if !key.is_empty() {
        assert!(
            url.query_pairs()
                .any(|(k, v)| k == key.as_str() && v == value.as_str()),
            "{}: {}",
            c.id,
            url
        );
    }
    if c.id == "opensharing" {
        assert!(url.query_pairs().any(|(k, v)| k == "fid[]" && v == "6"));
    }
}
#[test]
fn every_pinned_public_layout_extracts_fields_and_empty_results() {
    let mut failed = vec![];
    for c in cases() {
        let id = c.id.clone();
        if std::panic::catch_unwind(|| {
            let p = Plan::compile(&root(&c)).unwrap();
            // A download.selectors source keeps its plan-level download blocker; the
            // runtime resolves it (docs/download-selectors.md).
            assert!(
                p.blockers().iter().all(|b| {
                    c.detail.is_some() && b.path == "$.download"
                        || c.probe.is_some() && b.path == "$.login"
                }),
                "{}",
                c.id
            );
            let settings: BTreeMap<_, _> = c
                .settings
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let ctx = p.context(
                &settings,
                &query(&c),
                "2026-10-29T12:00:00Z".parse().unwrap(),
            );
            let requests = p.build_requests(&ctx).unwrap();
            assert_eq!(requests.len(), c.request_count, "{}", c.id);
            for request in &requests {
                request_matches(&c, &url::Url::parse(&request.url).unwrap());
                let body = page(&c);
                let rows = p
                    .extract(request, &body, &ctx)
                    .unwrap_or_else(|e| panic!("{}: {e}", c.id));
                assert_eq!(rows.len(), 1, "{}", c.id);
                let r = &rows[0];
                assert_eq!(r.title, c.title, "{}", c.id);
                assert_eq!(r.size, Some(c.expected_size.unwrap_or(1 << 30)), "{}", c.id);
                assert_eq!(r.seeders, Some(c.seeders), "{}", c.id);
                assert_eq!(r.peers, Some(c.peers), "{}", c.id);
                assert!(
                    r.categories.contains(&c.category),
                    "{}: {:?}",
                    c.id,
                    r.categories
                );
                assert_eq!(
                    r.published.as_deref(),
                    Some(c.published.as_str()),
                    "{}",
                    c.id
                );
                assert!(
                    r.details.as_ref().unwrap().ends_with(&c.details_path),
                    "{}",
                    c.id
                );
                if c.download_path == "magnet" {
                    assert!(
                        r.attributes.get("magnet").is_some_and(|v| v.contains(HASH))
                            || r.attributes.get("infohash").is_some_and(|v| v == HASH),
                        "{}: {:?}",
                        c.id,
                        r.attributes
                    );
                } else {
                    assert!(r.link.ends_with(&c.download_path), "{}: {}", c.id, r.link);
                }
                assert_eq!(r.attributes["downloadvolumefactor"], "0");
                assert_eq!(r.attributes["uploadvolumefactor"], "1");
                assert!(
                    p.extract(request, &empty(&c, &body), &ctx)
                        .unwrap()
                        .is_empty(),
                    "{}",
                    c.id
                );
                let broken = body
                    .replace("1 GB", "bad-size")
                    .replace("1 ГБ", "bad-size")
                    .replace("1073741824", "\"bad-size\"");
                let malformed = p.extract(request, &broken, &ctx);
                if c.fixed_size {
                    assert_eq!(malformed.unwrap()[0].size, c.expected_size, "{}", c.id);
                } else if c.optional_size {
                    assert_eq!(malformed.unwrap()[0].size, None, "{}", c.id);
                } else {
                    assert!(malformed.is_err(), "{}: malformed size accepted", c.id);
                }
            }
        })
        .is_err()
        {
            failed.push(id);
        }
    }
    assert!(failed.is_empty(), "Failed layouts: {failed:?}");
}

#[tokio::test]
async fn every_public_layout_runs_engine_cache_download_restart_errors_and_recovery() {
    use futures_util::{stream, StreamExt};
    // Every case runs to completion; failures are collected so one run lists them all.
    let failed: Vec<String> = stream::iter(cases().into_iter().filter(|c| c.id != "dmhy"))
        .map(|c| async move {
            let name = c.id.clone();
            let run = tokio::spawn(tokio::time::timeout(
                std::time::Duration::from_secs(180),
                lifecycle(c),
            ));
            match run.await {
                Ok(Ok(())) => None,
                Ok(Err(_)) => Some(format!("{name} (lifecycle deadline)")),
                Err(_) => Some(name),
            }
        })
        .buffer_unordered(9)
        .filter_map(|r| async move { r })
        .collect()
        .await;
    assert!(failed.is_empty(), "Failed lifecycles: {failed:?}");
}
async fn lifecycle(c: Case) {
    use axum::{
        extract::State,
        http::{HeaderMap, StatusCode, Uri},
        response::{IntoResponse, Response},
        routing::get,
        Router,
    };
    use rackett::engine::{Download, Engine};
    use std::sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Mutex,
    };
    use std::time::Instant;
    const TORRENT: &[u8] = b"d4:infod4:name7:fixture6:lengthi42eee";
    use rackett::cardigann_exec::request::Charset;
    // Non-UTF-8 definitions are served in their own code page, proving transcoding.
    let charset = rackett::cardigann_exec::Plan::compile(&root(&c))
        .unwrap()
        .charset();
    assert!(
        charset == Charset::Utf8 || !c.title.is_ascii(),
        "{}: a {} fixture needs a native non-ASCII title",
        c.id,
        charset.label()
    );
    struct Fixture {
        case: Case,
        body: String,
        charset: Charset,
        mode: AtomicU8,
        hits: Mutex<Vec<String>>,
    }
    impl Fixture {
        fn take(&self) -> Vec<String> {
            std::mem::take(&mut *self.hits.lock().unwrap())
        }
    }
    async fn respond(State(f): State<Arc<Fixture>>, uri: Uri, h: HeaderMap) -> Response {
        assert!(!h.contains_key("cookie"), "{}: cookie", f.case.id);
        assert!(
            !h.contains_key("authorization")
                || f.case.expected_headers.contains_key("authorization"),
            "{}: unexpected authorization",
            f.case.id
        );
        for (name, value) in &f.case.expected_headers {
            assert_eq!(
                h.get(name.as_str()).and_then(|v| v.to_str().ok()),
                Some(value.as_str()),
                "{}: header {name}",
                f.case.id
            );
        }
        if f.case.probe.as_deref() == Some(uri.to_string().as_str()) {
            return ([("content-type", "application/xml")], "<caps></caps>").into_response();
        }
        f.hits.lock().unwrap().push(uri.to_string());
        if let Some(detail) = &f.case.detail {
            if uri.to_string() == f.case.download_path {
                let page = f.charset.encode(&detail.html).unwrap_or_else(|| {
                    panic!("{}: detail text outside {}", f.case.id, f.charset.label())
                });
                return ([("content-type", "text/html")], page).into_response();
            }
            if uri.to_string() == detail.resolved {
                return ([("content-type", "application/x-bittorrent")], TORRENT).into_response();
            }
        } else if uri.to_string() == f.case.download_path {
            return ([("content-type", "application/x-bittorrent")], TORRENT).into_response();
        }
        let mode = f.mode.load(Ordering::SeqCst);
        if mode == 4 {
            return (StatusCode::SERVICE_UNAVAILABLE, "fixture-private-error").into_response();
        }
        if mode == 5 {
            return (
                StatusCode::FOUND,
                [("location", "http://127.0.0.1:1/fixture-private-error")],
            )
                .into_response();
        }
        let body = match mode {
            0 if f.case.id == "nipponsei"
                && url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
                    .filter(|(k, _)| k == "search")
                    .all(|(_, v)| v.is_empty()) =>
            {
                include_str!("fixtures/public-layouts/nipponsei-latest.html").into()
            }
            1 => empty(&f.case, &f.body),
            6 if f.case.id == "sukebeinyaasi" => f
                .body
                .replace("<a href=\"/download/42.torrent\">get</a>", ""),
            6 if f.case.id == "torrentbyte" => f.body.replace(HASH, &format!("|{}", &HASH[1..])),
            6 if f.case.id == "dmhy" => {
                let row = f
                    .body
                    .split_once("<!-- rows:start -->")
                    .unwrap()
                    .1
                    .split_once("<!-- rows:end -->")
                    .unwrap()
                    .0;
                f.body.replace(
                    "<!-- rows:end -->",
                    &format!(
                        "{}<!-- rows:end -->",
                        row.replace("<td>11</td>", "<td>-</td>")
                    ),
                )
            }
            7 if f.case.id == "dmhy" => f.body.replace("<td>11</td>", "<td>-</td>"),
            8 if f.case.id == "dmhy" => "<table id=topic_list><tbody></tbody></table>".into(),
            2 => {
                if f.case.json {
                    "{\"error\":\"fixture-private-error\"}".into()
                } else {
                    "<html><body>fixture-private-error</body></html>".into()
                }
            }
            _ => f.body.clone(),
        };
        let mut response = (
            [(
                "content-type",
                if f.case.json {
                    "application/json"
                } else {
                    "text/html"
                },
            )],
            // The header deliberately omits the charset: the definition encoding wins.
            f.charset.encode(&body).unwrap_or_else(|| {
                panic!("{}: fixture text outside {}", f.case.id, f.charset.label())
            }),
        )
            .into_response();
        if mode == 3 {
            response
                .headers_mut()
                .insert("cf-mitigated", "challenge".parse().unwrap());
        }
        response
    }
    let fixture = Arc::new(Fixture {
        case: c.clone(),
        body: page(&c),
        charset,
        mode: AtomicU8::new(0),
        hits: Default::default(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new()
        .fallback(get(respond))
        .with_state(fixture.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let runtime = builtin::Runtime::compile(&stock(&c, &base))
        .unwrap_or_else(|e| panic!("{} compile: {e}", c.id));
    assert_eq!(runtime.delay_ms, c.delay_ms, "{}", c.id);
    assert_eq!(runtime.empty_html_container, c.container, "{}", c.id);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let engine = Arc::new(Engine::new(path.clone()).await.unwrap());
    let i = engine.put_indexer(stock(&c, &base), false).await.unwrap();
    assert!(fixture.take().is_empty());
    let q = query(&c);
    let started = Instant::now();
    let first = engine.search(vec![i.id.clone()], q.clone()).await;
    let elapsed = started.elapsed();
    assert!(first.errors.is_empty(), "{}: {:?}", c.id, first.errors);
    assert_eq!(first.results.len(), 1, "{}", c.id);
    let r = &first.results[0];
    assert_eq!(r.title, c.title, "{}", c.id);
    assert_eq!(r.size, Some(c.expected_size.unwrap_or(1 << 30)));
    assert_eq!(r.seeders, Some(c.seeders));
    assert_eq!(r.peers, Some(c.peers));
    let hits = fixture.take();
    assert_eq!(hits.len(), c.request_count, "{}", c.id);
    for uri in &hits {
        request_matches(
            &c,
            &url::Url::parse(&format!("{}{}", base.trim_end_matches('/'), uri)).unwrap(),
        );
    }
    // The transport gates client dispatch, not server receipt. A busy executor
    // can delay receipt of the first request and compress the observed arrival
    // interval. The whole cold search must still pay every inter-page delay.
    let minimum_ms = c.delay_ms * c.request_count.saturating_sub(1) as u64;
    assert!(
        elapsed.as_millis() >= u128::from(minimum_ms.saturating_sub(25)),
        "{}: pinned cold-search delay bypass ({elapsed:?})",
        c.id
    );
    if c.request_count == 2 {
        assert_ne!(hits[0], hits[1], "{}: distinct pages", c.id);
    }
    let cached = engine.search(vec![i.id.clone()], q.clone()).await;
    assert_eq!(cached.cached, 1, "{}", c.id);
    assert_eq!(cached.results[0].title, c.title);
    assert!(fixture.take().is_empty());
    if c.id == "nipponsei" {
        let latest = engine
            .search(vec![i.id.clone()], SearchQuery::default())
            .await;
        assert!(latest.errors.is_empty(), "{:?}", latest.errors);
        let row = &latest.results[0];
        assert_eq!(
            (&row.title, row.seeders, row.peers, row.size),
            (&c.title, Some(11), Some(15), Some(1 << 30))
        );
        let hits = fixture.take();
        assert_eq!(hits.len(), 1);
        let url = url::Url::parse(&format!("{}{}", base.trim_end_matches('/'), hits[0])).unwrap();
        assert!(url
            .query_pairs()
            .filter(|(k, _)| k == "search")
            .all(|(_, v)| v.is_empty()));
    }
    let mut presented = first.clone();
    rackett::protocol::present_results(&mut presented, "fixture-key", "http://127.0.0.1:9117");
    // Configured secrets (an API key) never reach presented results or errors.
    for secret in c.settings.values().filter_map(|v| v.as_str()) {
        assert!(
            !serde_json::to_string(&presented).unwrap().contains(secret),
            "{}: secret in results",
            c.id
        );
    }
    if c.download_path == "magnet" {
        assert!(
            r.link.starts_with("magnet:?") && r.link.contains(HASH),
            "{}: {}",
            c.id,
            r.link
        );
        assert_eq!(presented.results[0].link, r.link);
    } else {
        assert!(presented.results[0]
            .link
            .starts_with("http://127.0.0.1:9117/dl/"));
        assert_eq!(
            engine
                .download(&i.id, r.source_epoch, &r.link)
                .await
                .unwrap_or_else(|e| panic!("{} download: {e}", c.id)),
            match c.detail.as_ref().map(|d| d.resolved.as_str()) {
                Some(magnet) if magnet.starts_with("magnet:") => Download::Magnet(magnet.into()),
                _ => Download::Torrent {
                    bytes: TORRENT.to_vec(),
                    content_type: "application/x-bittorrent".into(),
                },
            }
        );
        let hits = fixture.take();
        assert_eq!(hits[0], c.download_path);
        match &c.detail {
            Some(d) if !d.resolved.starts_with("magnet:") => {
                assert_eq!(
                    hits,
                    [c.download_path.clone(), d.resolved.clone()],
                    "{}",
                    c.id
                )
            }
            _ => assert_eq!(hits.len(), 1, "{}", c.id),
        }
    }
    assert!(engine
        .download(
            &i.id,
            r.source_epoch,
            "http://127.0.0.1:1/fixture-private-error"
        )
        .await
        .is_err());
    assert!(fixture.take().is_empty());
    engine
        .patch_indexer(&i.id, json!({"name":format!("Updated {}",c.id)}))
        .await
        .unwrap();
    if c.download_path != "magnet" {
        assert!(engine
            .download(&i.id, r.source_epoch, &r.link)
            .await
            .is_err());
        assert!(fixture.take().is_empty());
    }
    drop(engine);
    let engine = Arc::new(Engine::new(path.clone()).await.unwrap());
    assert_eq!(engine.indexers().await[0].definition, i.definition);
    let restarted = engine.search(vec![i.id.clone()], q.clone()).await;
    assert!(
        restarted.errors.is_empty(),
        "{}: {:?}",
        c.id,
        restarted.errors
    );
    assert_eq!(restarted.results.len(), 1);
    assert_eq!(restarted.cached, 0);
    assert_eq!(fixture.take().len(), c.request_count);
    if c.id == "dmhy" {
        for mode in [6, 7, 8] {
            engine.clear_cache().await;
            fixture.mode.store(mode, Ordering::SeqCst);
            let result = engine.search(vec![i.id.clone()], q.clone()).await;
            assert_eq!(
                result.results.len(),
                usize::from(mode == 6),
                "DMHY mode {mode}"
            );
            assert_eq!(result.errors.is_empty(), mode == 6, "DMHY mode {mode}");
            assert_eq!(fixture.take().len(), 1);
            if mode == 6 {
                let cached = engine.search(vec![i.id.clone()], q.clone()).await;
                assert_eq!((cached.cached, cached.results.len()), (1, 1));
                assert!(fixture.take().is_empty());
            }
        }
    }
    for mode in [1, 2, 3, 4, 5] {
        engine.clear_cache().await;
        fixture.mode.store(mode, Ordering::SeqCst);
        let result = engine.search(vec![i.id.clone()], q.clone()).await;
        assert!(result.results.is_empty(), "{} mode {mode}", c.id);
        assert_eq!(
            result.errors.is_empty(),
            mode == 1,
            "{} mode {mode}: {:?}",
            c.id,
            result.errors
        );
        let text = serde_json::to_string(&result).unwrap();
        assert!(!text.contains("fixture-private-error"), "{}", c.id);
        let hits = fixture.take();
        assert_eq!(
            hits.len(),
            if mode == 1 { c.request_count } else { 1 },
            "{} mode {mode}",
            c.id
        );
    }
    fixture.mode.store(0, Ordering::SeqCst);
    let recovered = engine.search(vec![i.id.clone()], q).await;
    assert!(
        recovered.errors.is_empty(),
        "{}: {:?}",
        c.id,
        recovered.errors
    );
    assert_eq!(recovered.results.len(), 1);
    assert_eq!(fixture.take().len(), c.request_count);
    if c.id == "sukebeinyaasi" || c.id == "torrentbyte" {
        engine.clear_cache().await;
        fixture.mode.store(6, Ordering::SeqCst);
        let special = engine.search(vec![i.id.clone()], query(&c)).await;
        if c.id == "sukebeinyaasi" {
            assert!(special.errors.is_empty(), "{:?}", special.errors);
            assert!(special.results[0].link.starts_with("magnet:?"));
        } else {
            assert!(special.results.is_empty());
            assert_eq!(special.errors.len(), 1);
            assert_eq!(special.errors[0].message, "Invalid release infohash");
        }
        assert_eq!(fixture.take().len(), 1);
    }
    let bad = SearchQuery {
        params: BTreeMap::from([("t".into(), "unsupported".into())]),
    };
    let rejected = engine.search(vec![i.id.clone()], bad).await;
    assert!(!rejected.errors.is_empty());
    assert!(fixture.take().is_empty());
    engine.delete_indexer(&i.id).await.unwrap();
    assert!(engine.indexers().await.is_empty());
    drop(engine);
    assert!(Engine::new(path).await.unwrap().indexers().await.is_empty());
    server.abort();
}

#[test]
fn pinned_public_settings_queries_and_optional_fields_are_preserved() {
    for c in cases() {
        let p = Plan::compile(&root(&c)).unwrap();
        let now = "2026-10-29T12:00:00Z".parse().unwrap();
        let configured: BTreeMap<_, _> = c
            .settings
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let ctx = p.context(&configured, &SearchQuery::default(), now);
        let requests = p.build_requests(&ctx).unwrap();
        assert_eq!(
            requests.len(),
            c.keywordless_request_count.unwrap_or(c.request_count),
            "{}",
            c.id
        );
        let url = url::Url::parse(&requests[0].url).unwrap();
        match c.id.as_str() {
            "sexypics" => assert_eq!(url.path(), "/browse/all/age/desc/"),
            "extratorrent-st" => assert_eq!(url.path(), "/"),
            "kickasstorrents-to" => assert_eq!(url.path(), "/17/all/"),
            "kickasstorrents-ws" => assert_eq!(url.path(), "/new/"),
            "mikan" => assert_eq!(url.path(), "/Home/Classic"),
            "therarbg" => assert!(url.path().contains(":time:10D:")),
            "internetarchive" => assert!(url
                .query_pairs()
                .any(|(k, v)| k == "q" && v == "format:(\"Archive BitTorrent\")")),
            "opensharing" => assert!(url
                .query_pairs()
                .any(|(k, v)| k == "keywords" && v == "2026")),
            _ => assert_eq!(url.path(), c.search_path),
        }
        let mut settings: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        match c.id.as_str() {
            "kickasstorrents-ws" => {
                settings.insert("sort".into(), json!("size"));
                settings.insert("type".into(), json!("asc"));
            }
            "newstudio" => {
                settings.insert("sort".into(), json!("10"));
                settings.insert("type".into(), json!("1"));
                settings.insert("addrussiantotitle".into(), json!(true));
            }
            "therarbg" => {
                settings.insert("sort".into(), json!("-se"));
            }
            "internetarchive" => {
                settings.insert("titleOnly".into(), json!(false));
                settings.insert("noMagnet".into(), json!(true));
                settings.insert("sort".into(), json!("downloads"));
                settings.insert("type".into(), json!("_"));
            }
            "opensharing" => {
                settings.insert("sort".into(), json!("ts"));
                settings.insert("type".into(), json!("a"));
            }
            _ => {}
        }
        let mut i = stock(&c, "http://127.0.0.1:1/");
        i.definition.as_mut().unwrap()["settings"] = json!(settings);
        let rt = builtin::Runtime::compile(&i).unwrap();
        let ctx = rt.plan.context(&rt.settings, &query(&c), now);
        let req = rt.plan.build_requests(&ctx).unwrap().remove(0);
        let url = url::Url::parse(&req.url).unwrap();
        let pairs: BTreeMap<_, _> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let rows = rt.plan.extract(&req, &page(&c), &ctx).unwrap();
        match c.id.as_str() {
            "kickasstorrents-ws" => {
                assert_eq!(pairs["field"], "size");
                assert_eq!(pairs["sorder"], "asc");
                let body = page(&c)
                    .replace("<td>11</td>", "<td>N/A</td>")
                    .replace("<td>4</td>", "<td>N/A</td>");
                let rows = rt.plan.extract(&req, &body, &ctx).unwrap();
                assert_eq!((rows[0].seeders, rows[0].peers), (Some(0), Some(0)));
            }
            "newstudio" => {
                assert_eq!(pairs["o"], "10");
                assert_eq!(pairs["s"], "1");
                assert_eq!(rows[0].title, "Fixture Release RUS-NewStudio");
            }
            "therarbg" => {
                assert!(url.path().contains("order:-se:"));
                assert_eq!(rows[0].attributes["imdbid"], "1234567");
            }
            "internetarchive" => {
                assert_eq!(pairs["sort"], "downloads");
                assert_eq!(
                    pairs["q"],
                    "Fixture AND format:(\"Archive BitTorrent\") AND mediatype:(movies)"
                );
                assert!(rows[0]
                    .attributes
                    .get("infohash")
                    .is_none_or(|v| v.is_empty()));
                let body = page(&c).replace("\"title\": \"Fixture Release\",", "");
                assert_eq!(
                    rt.plan.extract(&req, &body, &ctx).unwrap()[0].title,
                    "Missing title for fixture"
                );
            }
            "opensharing" => {
                assert_eq!(pairs["sk"], "ts");
                assert_eq!(pairs["sd"], "a");
            }
            "mikan" => assert!(rows[0].attributes["magnet"].contains(HASH)),
            _ => {}
        }
        if !settings.is_empty() {
            let key = settings.keys().next().unwrap().clone();
            i.definition.as_mut().unwrap()["settings"][key] = json!(["invalid-fixture"]);
            assert!(builtin::Runtime::compile(&i).is_err(), "{}", c.id);
        }
    }
}

#[tokio::test]
async fn infohash_only_runtime_rejects_injection_and_accepts_exact_btih_encodings() {
    use axum::{extract::State, routing::get, Router};
    use rackett::engine::Engine;
    use std::sync::{Arc, Mutex};
    let c = cases().into_iter().find(|c| c.id == "therarbg").unwrap();
    let body = Arc::new(Mutex::new(page(&c)));
    let app = Router::new()
        .fallback(get(|State(body): State<Arc<Mutex<String>>>| async move {
            (
                [("content-type", "application/json")],
                body.lock().unwrap().clone(),
            )
        }))
        .with_state(body.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let engine = Arc::new(Engine::new(dir.path().join("config.json")).await.unwrap());
    let i = engine.put_indexer(stock(&c, &base), false).await.unwrap();
    for hash in [
        HASH.to_string(),
        HASH.to_uppercase(),
        "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567".into(),
        "abcdefghijklmnopqrstuvwxyz234567".into(),
        "".into(),
        "a".repeat(39),
        "a".repeat(41),
        "g".repeat(40),
        "0".repeat(32),
        format!("{HASH}&tr=https://fixture-private-error"),
        format!("{HASH}#fragment"),
        format!(" {HASH}"),
        "é".repeat(20),
    ] {
        // The interpreter trims field text before transport validation.
        let valid = hash.trim() == HASH
            || hash == HASH.to_uppercase()
            || hash == "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567"
            || hash == "abcdefghijklmnopqrstuvwxyz234567";
        let mut value: serde_json::Value = serde_json::from_str(&page(&c)).unwrap();
        value["results"][0]["h"] = json!(hash);
        *body.lock().unwrap() = value.to_string();
        engine.clear_cache().await;
        let result = engine.search(vec![i.id.clone()], query(&c)).await;
        if valid {
            assert!(result.errors.is_empty(), "{:?}", result.errors);
            assert_eq!(
                result.results[0].link,
                format!("magnet:?xt=urn:btih:{}", hash.trim())
            );
        } else {
            assert!(result.results.is_empty(), "invalid hash {hash:?} accepted");
            assert_eq!(result.errors.len(), 1);
            assert!(!serde_json::to_string(&result.errors)
                .unwrap()
                .contains("fixture-private-error"));
        }
    }
    server.abort();
}

#[test]
fn additional_public_layout_branches_match_pinned_filters() {
    let now = "2026-10-29T12:00:00Z".parse().unwrap();
    for c in cases().into_iter().filter(|c| {
        [
            "shanaproject",
            "nipponsei",
            "sukebeinyaasi",
            "torrentbyte",
            "rutracker-ru",
            "extratorrent-st",
            "u3c3",
            "sexypics",
            "torrent-pirat",
            "xxxtor",
        ]
        .contains(&c.id.as_str())
    }) {
        let p = Plan::compile(&root(&c)).unwrap();
        let mut q = query(&c);
        let mut settings: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        match c.id.as_str() {
            "shanaproject" => {
                q.params.insert("q".into(), "Fixture S01E02".into());
                settings.insert("sort".into(), json!("fsize"));
                settings.insert("type".into(), json!("Ascending"));
            }
            "sukebeinyaasi" => {
                settings.insert("filter-id".into(), json!("2"));
                settings.insert("cat-id".into(), json!("1_0"));
                settings.insert("sort".into(), json!("seeders"));
                settings.insert("type".into(), json!("asc"));
            }
            "rutracker-ru" => {
                q.params.insert("q".into(), "Fíxture S01E02".into());
                settings.insert("uploader".into(), json!("Fixture Author"));
                settings.insert("stripcyrillic".into(), json!(true));
                settings.insert("addrussiantotitle".into(), json!(true));
            }
            "extratorrent-st" => {
                q.params.insert("q".into(), "Fixture Release".into());
            }
            "sexypics" => {
                q.params.insert("q".into(), "Fixture Release".into());
                settings.insert("sort".into(), json!("se"));
                settings.insert("type".into(), json!("asc"));
            }
            "torrent-pirat" => {
                settings.insert("sort".into(), json!("ts"));
                settings.insert("type".into(), json!("a"));
            }
            _ => {}
        }
        let mut source = stock(&c, "http://127.0.0.1:1/");
        source.definition.as_mut().unwrap()["settings"] = json!(settings);
        let rt = builtin::Runtime::compile(&source).unwrap();
        let ctx = p.context(&rt.settings, &q, now);
        let request = p.build_requests(&ctx).unwrap().remove(0);
        let url = url::Url::parse(&request.url).unwrap();
        let pairs: BTreeMap<_, _> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let body = page(&c);
        match c.id.as_str() {
            "shanaproject" => {
                assert_eq!(pairs["title"], "Fixture");
                assert_eq!(pairs["sort"], "fsize");
                assert_eq!(pairs["dir"], "Ascending");
                for (text, date) in [
                    ("Oct 1, 2026, 1 p.m.", "2026-10-01T13:00:00Z"),
                    ("Oct 1, 2026, noon", "2026-10-01T12:00:00Z"),
                    ("unknown date", "2026-10-29T12:00:00Z"),
                ] {
                    let alternate = body.replace("Oct 1, 2026, 12:34 p.m.", text);
                    assert_eq!(
                        p.extract(&request, &alternate, &ctx).unwrap()[0]
                            .published
                            .as_deref(),
                        Some(date)
                    );
                }
            }
            "nipponsei" => {
                let latest=body.replace("<td>11</td><td>4</td><td class=\"date\">2026-10-01 12:34</td><td class=\"bytes\">1 GB</td>","<td class=\"date\">2026-10-01 12:34</td><td class=\"bytes\">1 GB</td><td>11</td><td>4</td>");
                assert_ne!(latest, body);
                let empty_ctx = p.context(&rt.settings, &SearchQuery::default(), now);
                let req = p.build_requests(&empty_ctx).unwrap().remove(0);
                let r = p.extract(&req, &latest, &empty_ctx).unwrap().remove(0);
                assert_eq!(
                    (r.seeders, r.peers, r.size),
                    (Some(11), Some(15), Some(1 << 30))
                );
                let missing = body
                    .replace("class=\"date\"", "class=\"missing-date\"")
                    .replace("class=\"bytes\"", "class=\"missing-size\"");
                let r = p.extract(&request, &missing, &ctx).unwrap().remove(0);
                assert_eq!(r.size, Some(128 << 20));
                assert_eq!(r.published.as_deref(), Some("2026-10-29T12:00:00Z"));
            }
            "sukebeinyaasi" => {
                assert_eq!(pairs["f"], "2");
                assert_eq!(pairs["c"], "1_0");
                assert_eq!(pairs["s"], "seeders");
                assert_eq!(pairs["o"], "asc");
                for class in ["default", "danger", "success"] {
                    assert_eq!(
                        p.extract(
                            &request,
                            &body.replace("class=\"success\"", &format!("class=\"{class}\"")),
                            &ctx
                        )
                        .unwrap()
                        .len(),
                        1
                    );
                }
            }
            "torrentbyte" => {
                let r = p.extract(&request, &body, &ctx).unwrap().remove(0);
                assert_eq!(r.attributes["files"], "3");
                assert_eq!(r.attributes["description"], "Fixture description");
                let empty="<table><tbody><tr><td>No matching torrent records found.</td></tr></tbody></table>";
                assert!(p.extract(&request, empty, &ctx).unwrap().is_empty());
            }
            "rutracker-ru" => {
                assert_eq!(pairs["nm"], "Fixture сезон 1 сери 2");
                assert_eq!(pairs["pn"], "Fixture Author");
                let r = p
                    .extract(
                        &request,
                        &body.replace("Fixture Release", "Fixture Сезон 1 Серия 2"),
                        &ctx,
                    )
                    .unwrap()
                    .remove(0);
                assert_eq!(r.title, "Fixture S1E2 RUS");
            }
            "extratorrent-st" => {
                assert_eq!(pairs["search"], "Fixture.Release");
                let r = p
                    .extract(
                        &request,
                        &body
                            .replace("class=\"sy\"", "class=\"missing\"")
                            .replace("class=\"ly\"", "class=\"missing\""),
                        &ctx,
                    )
                    .unwrap()
                    .remove(0);
                assert_eq!((r.seeders, r.peers), (Some(0), Some(0)));
            }
            "u3c3" => {
                assert_eq!(pairs["search2"], "eelja3lfe1a1");
            }
            "sexypics" => assert_eq!(url.path(), "/f/fixture-release/se/asc/"),
            "torrent-pirat" => {
                assert_eq!(pairs["sk"], "ts");
                assert_eq!(pairs["sd"], "a");
                assert_eq!(pairs["fp"], "1");
            }
            _ => {}
        }
    }
}

#[test]
fn dmhy_empty_marker_requires_exact_visible_leaf_text_not_a_table() {
    use rackett::cardigann_exec::query_leaf_text_equals;
    const SEL: &str = r#"div[style="text-align: center;padding: 5px;background-color: #CCDDFF"]"#;
    const TEXT: &str = "沒有可顯示資源";
    let empty = include_str!("fixtures/public-layouts/dmhy-empty.html");
    assert_eq!(query_leaf_text_equals(empty, SEL, TEXT), Ok(true));
    assert_eq!(
        query_leaf_text_equals(&empty.replace(TEXT, &format!(" \n{TEXT}\t ")), SEL, TEXT),
        Ok(true)
    );
    for body in [
        "<table class=tablesorter id=topic_list><tbody></tbody></table>".to_owned(),
        format!("<template>{empty}</template>"),
        empty.replace(TEXT, &format!("Not {TEXT}")),
        empty.replace(TEXT, &format!("{TEXT}: Login required")),
        empty.replace(TEXT, &format!("<span>{TEXT}</span>")),
        empty.replace(TEXT, ""),
        format!("<script>{empty}</script>"),
    ] {
        assert_eq!(query_leaf_text_equals(&body, SEL, TEXT), Ok(false));
    }
    assert_eq!(
        query_leaf_text_equals(
            &format!("<select><option>Filter</option></select>{empty}"),
            SEL,
            TEXT
        ),
        Ok(true)
    );
    assert!(query_leaf_text_equals(
        &format!("<div><template shadowrootmode=open></template></div>{empty}"),
        SEL,
        TEXT
    )
    .is_err());
    assert!(query_leaf_text_equals(&empty.replace(TEXT, &"x".repeat(4097)), SEL, TEXT).is_err());
    assert!(query_leaf_text_equals(empty, "div > div", TEXT).is_err());
}

/// Opt-in replay of redacted/local captures: no requests, credentials, or fixture
/// substitution. Capture hashes and observed response metadata are documented.
#[test]
#[ignore = "Requires local DMHY result and no-match captures; never contacts a tracker"]
fn replay_operator_supplied_dmhy_captures() {
    let positive = std::fs::read_to_string(
        std::env::var("RACKETT_DMHY_RESULTS_CAPTURE").expect("set results capture path"),
    )
    .unwrap();
    let empty = std::fs::read_to_string(
        std::env::var("RACKETT_DMHY_EMPTY_CAPTURE").expect("set empty capture path"),
    )
    .unwrap();
    let c = cases().into_iter().find(|c| c.id == "dmhy").unwrap();
    let p = Plan::compile(&root(&c)).unwrap();
    let ctx = p.context(
        &BTreeMap::new(),
        &SearchQuery::default(),
        chrono::Utc::now(),
    );
    let req = p.build_requests(&ctx).unwrap().remove(0);
    let meta = rackett::cardigann_exec::ResponseMeta {
        status: 200,
        redirected: false,
        refresh: false,
        body_bytes: positive.len(),
    };
    let rows = p
        .extract_response_skipping_invalid_html_rows(&req, &meta, &positive, &ctx)
        .unwrap();
    assert!(!rows.is_empty());
    assert!(p.extract(&req, &empty, &ctx).unwrap().is_empty());
    assert_eq!(
        rackett::cardigann_exec::query_leaf_text_equals(
            &empty,
            r#"div[style="text-align: center;padding: 5px;background-color: #CCDDFF"]"#,
            "沒有可顯示資源"
        ),
        Ok(true)
    );
    println!(
        "Extracted {} actual captured rows; exact no-match marker verified",
        rows.len()
    );
}

#[tokio::test]
async fn dmhy_runtime_keeps_valid_rows_and_requires_explicit_empty_evidence() {
    let c = cases().into_iter().find(|c| c.id == "dmhy").unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(60), lifecycle(c))
        .await
        .unwrap();
}
