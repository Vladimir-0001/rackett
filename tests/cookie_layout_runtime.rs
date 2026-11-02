//! Independent, source-pinned HTML layouts and full supplied-cookie lifecycles.
//! Synthetic fixtures establish runtime support, never live account availability.
use rackett::{builtin, cardigann, cardigann_exec::Plan, models::SearchQuery};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

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
    /// The search response is JSON (`response: {type: json}`): the fixture file is the
    /// JSON body (no rows markers) and `empty_body` is the source's empty result.
    #[serde(default)]
    json: bool,
    #[serde(default)]
    empty_body: Option<String>,
    #[serde(default)]
    detail: Option<Detail>,
    /// The definition declares no `login.test`: pinned TestLogin sends no validation
    /// request and only a search redirect means expiry (docs/login-without-test.md).
    /// `login_path` is then unused.
    #[serde(default)]
    no_test: bool,
    sha256: String,
    category: u32,
    custom_category: Option<u32>,
    missing_title: Option<String>,
    /// Required, and non-ASCII, for non-UTF-8 definitions to prove transcoding.
    expected_title: Option<String>,
    request_count: usize,
    login_path: String,
    search_path: String,
    #[serde(default)]
    search_paths: Vec<String>,
    #[serde(default)]
    user_agent: bool,
    /// The definition declares `Referer: {{ .Config.sitelink }}<suffix>`: login tests
    /// and searches carry it (docs/session-search-headers.md).
    #[serde(default)]
    referer: Option<String>,
    /// The definition declares this literal `Cookie` search header: every request
    /// sends it first, then `"; "` and the session cookie.
    #[serde(default)]
    literal_cookie: Option<String>,
    #[serde(default = "one")]
    normal_download: f64,
    #[serde(default = "one")]
    normal_upload: f64,
    keyword: String,
    keyword_value: String,
    category_input: Option<[String; 2]>,
    /// The definition searches with `method: post`; keyword and category pairs are
    /// matched in the form body, decoded with the definition charset.
    #[serde(default)]
    search_post: bool,
    download_path: String,
    details_path: String,
    published: String,
    free_markup: String,
    normal_markup: String,
    free_download: f64,
    free_upload: f64,
}
fn one() -> f64 {
    1.0
}
const AGENT: &str = "Fixture Browser/1.0";
const AGENT_CHANGED: &str = "Fixture Browser/2.0";
/// Cookie layout cases: the original manifest plus independently merged batches.
const COOKIE_MANIFESTS: [&str; 7] = [
    include_str!("fixtures/cookie-layouts/cases.json"),
    include_str!("fixtures/cookie-batches/batch1.json"),
    include_str!("fixtures/cookie-batches/batch2.json"),
    include_str!("fixtures/cookie-batches/batch3.json"),
    include_str!("fixtures/cookie-batches/batch4.json"),
    include_str!("fixtures/cookie-batches/batch5.json"),
    include_str!("fixtures/cookie-batches/batch6.json"),
];
fn cases() -> Vec<Case> {
    let mut cases: Vec<Case> = COOKIE_MANIFESTS
        .iter()
        .flat_map(|m| serde_json::from_str::<Vec<Case>>(m).unwrap())
        .collect();
    // `RACKETT_COOKIE_CASES=matrix,hdtime` re-runs single lifecycles locally.
    if let Ok(only) = std::env::var("RACKETT_COOKIE_CASES") {
        let only: Vec<&str> = only.split(',').collect();
        cases.retain(|c| only.contains(&c.id.as_str()));
    }
    cases
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
fn page(c: &Case, free: bool) -> String {
    std::fs::read_to_string(format!("tests/fixtures/cookie-layouts/{}.html", c.id))
        .unwrap()
        .replace(
            "@@PROMO@@",
            if free {
                &c.free_markup
            } else {
                &c.normal_markup
            },
        )
}
fn empty_for(c: &Case, page: &str) -> String {
    match (&c.empty_body, c.json) {
        (Some(body), true) => body.clone(),
        _ => empty(page),
    }
}
fn empty(page: &str) -> String {
    let (before, rest) = page.split_once("<!-- rows:start -->").unwrap();
    let (_, after) = rest.split_once("<!-- rows:end -->").unwrap();
    format!("{before}{after}")
}
/// Extraction as the session runtime sees a plain HTTP 200 response, so definitions
/// with an empty `noResultsMessage` (superbits) get their transport metadata.
fn extract(
    p: &Plan,
    request: &rackett::cardigann_exec::request::BuiltRequest,
    body: &str,
    ctx: &rackett::cardigann_exec::template::Context,
) -> Result<Vec<rackett::models::Release>, rackett::cardigann_exec::ExecutionError> {
    let meta = rackett::cardigann_exec::ResponseMeta {
        status: 200,
        redirected: false,
        refresh: false,
        body_bytes: body.len(),
    };
    p.extract_response(request, &meta, body, ctx)
}
fn query(c: &Case) -> SearchQuery {
    SearchQuery {
        params: BTreeMap::from([
            ("q".into(), "Fixture".into()),
            ("cat".into(), c.category.to_string()),
        ]),
    }
}
/// The request URL with a POST search body folded into its query, so one matcher
/// covers both shapes. A non-UTF-8 body must use HttpUtility.UrlEncode's lower-case
/// escapes, and its bytes decode with the definition charset.
fn search_url(
    c: &Case,
    charset: rackett::cardigann_exec::request::Charset,
    uri: &str,
    body: &[u8],
) -> url::Url {
    let mut url = url::Url::parse(uri).unwrap();
    if !c.search_post {
        assert!(body.is_empty(), "{}: unexpected search body", c.id);
        return url;
    }
    let pairs: Vec<(String, String)> = if charset == rackett::cardigann_exec::request::Charset::Utf8
    {
        url::form_urlencoded::parse(body).into_owned().collect()
    } else {
        let text = std::str::from_utf8(body).expect("percent-encoded body is ASCII");
        for escape in text.split('%').skip(1) {
            assert!(
                escape.len() >= 2
                    && escape[..2]
                        .bytes()
                        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
                "{}: non-HttpUtility escape in {text}",
                c.id
            );
        }
        let decode = |part: &str| {
            let raw = part.replace('+', " ");
            let mut bytes = Vec::new();
            let mut i = 0;
            while i < raw.len() {
                if raw.as_bytes()[i] == b'%' {
                    bytes.push(u8::from_str_radix(&raw[i + 1..i + 3], 16).unwrap());
                    i += 3;
                } else {
                    bytes.push(raw.as_bytes()[i]);
                    i += 1;
                }
            }
            charset.decode(bytes).unwrap()
        };
        text.split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| {
                let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                (decode(k), decode(v))
            })
            .collect()
    };
    url.query_pairs_mut().extend_pairs(pairs);
    url
}
/// `search_url` for a built request; GET requests must carry no body.
fn built_url(
    c: &Case,
    p: &Plan,
    request: &rackett::cardigann_exec::request::BuiltRequest,
) -> url::Url {
    assert_eq!(
        request.method,
        if c.search_post { "POST" } else { "GET" },
        "{}",
        c.id
    );
    search_url(
        c,
        p.charset(),
        &request.url,
        request.body.as_deref().unwrap_or_default().as_bytes(),
    )
}
fn request_matches(c: &Case, url: &url::Url) {
    assert!(
        url.path() == c.search_path || c.search_paths.iter().any(|p| p == url.path()),
        "{}: path {}",
        c.id,
        url.path()
    );
    let pairs: Vec<_> = url.query_pairs().collect();
    assert!(
        pairs
            .iter()
            .any(|(k, v)| k == c.keyword.as_str() && v == c.keyword_value.as_str()),
        "{}: keyword",
        c.id
    );
    if let Some([key, value]) = &c.category_input {
        assert!(
            pairs
                .iter()
                .any(|(k, v)| k == key.as_str() && v == value.as_str()),
            "{}: category {key}={value}",
            c.id
        );
    }
    assert!(!url.as_str().contains("fixture-cookie"));
}
#[test]
fn pinned_layouts_extract_fields_promotions_and_empty_results() {
    let mut failures = Vec::new();
    for c in cases() {
        let id = c.id.clone();
        if std::panic::catch_unwind(|| check_layout(c)).is_err() {
            failures.push(id);
        }
    }
    assert!(failures.is_empty(), "Failed source layouts: {failures:?}");
}
fn check_layout(c: Case) {
    let p = Plan::compile(&root(&c)).unwrap_or_else(|e| panic!("{}: {e}", c.id));
    let now = "2026-10-29T12:00:00Z".parse().unwrap();
    let ctx = p.context(&BTreeMap::new(), &query(&c), now);
    let requests = p.build_requests(&ctx).unwrap();
    assert_eq!(requests.len(), c.request_count, "{}", c.id);
    let request = &requests[0];
    request_matches(&c, &built_url(&c, &p, request));
    for free in [false, true] {
        let body = page(&c, free);
        let rows = extract(&p, request, &body, &ctx)
            .unwrap_or_else(|e| panic!("{} free={free}: {e}", c.id));
        assert_eq!(rows.len(), 1, "{}", c.id);
        let r = &rows[0];
        assert_eq!(
            r.title,
            c.expected_title.as_deref().unwrap_or("Fixture Release"),
            "{}",
            c.id
        );
        assert_eq!(r.size, Some(1 << 30), "{}", c.id);
        assert_eq!((r.seeders, r.peers), (Some(11), Some(15)), "{}", c.id);
        assert!(r.categories.contains(&c.category), "{}", c.id);
        if let Some(category) = c.custom_category {
            assert!(r.categories.contains(&category), "{}", c.id);
        }
        assert_eq!(
            r.published.as_deref(),
            Some(c.published.as_str()),
            "{}",
            c.id
        );
        assert!(r.link.ends_with(&c.download_path), "{}: download", c.id);
        assert!(
            r.details.as_deref().unwrap().ends_with(&c.details_path),
            "{}: details",
            c.id
        );
        assert_eq!(
            r.attributes["downloadvolumefactor"].parse::<f64>().unwrap(),
            if free {
                c.free_download
            } else {
                c.normal_download
            },
            "{}",
            c.id
        );
        assert_eq!(
            r.attributes["uploadvolumefactor"].parse::<f64>().unwrap(),
            if free { c.free_upload } else { c.normal_upload },
            "{}",
            c.id
        );
        assert!(
            extract(&p, request, &empty_for(&c, &body), &ctx)
                .unwrap()
                .is_empty(),
            "{}",
            c.id
        );
        let mut untitled = body
            .replace("Fixture Release", "")
            .replace("Fixture%20Release", "");
        if let Some(title) = &c.expected_title {
            untitled = untitled.replace(title.as_str(), "");
        }
        let no_title = extract(&p, request, &untitled, &ctx);
        if let Some(fallback) = &c.missing_title {
            assert_eq!(no_title.unwrap()[0].title, *fallback, "{}", c.id);
        } else if let Ok(rows) = no_title {
            // A declared row filter (andmatch) may drop the untitled row instead.
            assert!(rows.is_empty(), "{}: empty title accepted", c.id);
        }
    }
}
#[tokio::test]
async fn every_layout_runs_through_cookie_engine_cache_download_edit_and_restart() {
    use futures_util::{stream, StreamExt};
    // Every case runs to completion; failures are collected so one run lists them all.
    let failed: Vec<String> = stream::iter(cases())
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
        // As in the form harness: pinned delays make lifecycles long, and more parallel
        // cases on a shared host push searches past their deadline.
        .buffer_unordered(4)
        .filter_map(|r| async move { r })
        .collect()
        .await;
    assert!(failed.is_empty(), "Failed lifecycles: {failed:?}");
}
async fn lifecycle(c: Case) {
    use axum::{
        body::Bytes,
        extract::State,
        http::{HeaderMap, Method, StatusCode, Uri},
        response::{Html, IntoResponse},
        routing::any,
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
    const SEED: &str = "session=fixture-cookie-seed";
    const RENEWED: &str = "session=fixture-cookie-renewed";
    const TORRENT: &[u8] = b"d4:infod4:name7:fixture6:lengthi42eee";
    use rackett::cardigann_exec::request::Charset;
    let charset = Plan::compile(&root(&c)).unwrap().charset();
    if charset != Charset::Utf8 {
        assert!(
            c.expected_title.as_deref().is_some_and(|t| !t.is_ascii()),
            "{}: a {} fixture needs a native non-ASCII expected_title",
            c.id,
            charset.label()
        );
    }
    struct Fixture {
        case: Case,
        base: String,
        body: String,
        charset: Charset,
        mode: AtomicU8,
        agent_changed: AtomicU8,
        hits: Mutex<Vec<(String, String)>>,
    }
    impl Fixture {
        fn take(&self) -> Vec<(String, String)> {
            std::mem::take(&mut *self.hits.lock().unwrap())
        }
        /// Serves HTML in the definition charset. Non-UTF-8 pages deliberately claim
        /// UTF-8 in the header: the pinned WebResult uses the definition encoding.
        fn html(&self, page: impl Into<String>) -> axum::response::Response {
            let page = page.into();
            if self.charset == Charset::Utf8 {
                return Html(page).into_response();
            }
            let bytes = self.charset.encode(&page).unwrap_or_else(|| {
                panic!(
                    "{}: fixture text outside {}",
                    self.case.id,
                    self.charset.label()
                )
            });
            ([("content-type", "text/html; charset=utf-8")], bytes).into_response()
        }
    }
    async fn respond(
        State(f): State<Arc<Fixture>>,
        method: Method,
        uri: Uri,
        h: HeaderMap,
        body: Bytes,
    ) -> axum::response::Response {
        // The engine addresses the fixture as 127.0.0.1. Local port scanners on a shared
        // machine (seen as `Host: localhost:<port>`, `user-agent: node`) are not
        // engine requests and must not count as hits.
        if !h
            .get("host")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|host| host.starts_with("127.0.0.1:"))
        {
            return StatusCode::NOT_FOUND.into_response();
        }
        let expected_agent = if !f.case.user_agent {
            "Rackett/0.1"
        } else if f.agent_changed.load(Ordering::SeqCst) == 0 {
            AGENT
        } else {
            AGENT_CHANGED
        };
        assert_eq!(
            h.get("user-agent").and_then(|h| h.to_str().ok()),
            Some(expected_agent),
            "{}: User-Agent",
            f.case.id
        );
        let target = f
            .case
            .detail
            .as_ref()
            .map_or(&f.case.download_path, |d| &d.resolved);
        // Pinned BaseIndexer.Download sends the link as Referer (`BaseIndexer.cs:548`).
        let referer = if uri.to_string() == *target {
            Some(format!("{}{}", f.base.trim_end_matches('/'), uri))
        } else if f.case.detail.is_some() && uri.to_string() == f.case.download_path {
            None
        } else {
            f.case.referer.as_ref().map(|r| format!("{}{r}", f.base))
        };
        assert_eq!(
            h.get("referer").and_then(|h| h.to_str().ok()),
            referer.as_deref(),
            "{}: Referer {uri}",
            f.case.id
        );
        let mut cookie = h.get("cookie").and_then(|v| v.to_str().ok()).unwrap_or("");
        if let Some(literal) = &f.case.literal_cookie {
            cookie = cookie
                .strip_prefix(literal.as_str())
                .and_then(|c| c.strip_prefix("; "))
                .unwrap_or_else(|| panic!("{}: literal Cookie first", f.case.id));
        }
        // POST search bodies are folded into the recorded query (`search_url`).
        let hit = if method == Method::POST {
            assert!(
                f.case.search_post && uri.path() == f.case.search_path,
                "{}: unexpected POST {uri}",
                f.case.id
            );
            assert_eq!(
                h.get("content-type").and_then(|v| v.to_str().ok()),
                Some("application/x-www-form-urlencoded"),
                "{}: search POST content type",
                f.case.id
            );
            let url = search_url(&f.case, f.charset, &format!("http://fixture{uri}"), &body);
            url[url::Position::BeforePath..].to_owned()
        } else {
            assert_eq!(method, Method::GET, "{}: method", f.case.id);
            uri.to_string()
        };
        f.hits.lock().unwrap().push((hit, cookie.into()));
        if cookie != SEED && cookie != RENEWED {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let mode = f.mode.load(Ordering::SeqCst);
        if mode == 2 {
            return f.html("<form>Sign in fixture-private-error</form>");
        }
        if mode == 4 {
            return (StatusCode::SERVICE_UNAVAILABLE, "fixture-private-error").into_response();
        }
        if mode == 5 {
            let mut r = StatusCode::FOUND.into_response();
            r.headers_mut()
                .insert("location", "/fixture-login".parse().unwrap());
            return r;
        }
        if let Some(detail) = &f.case.detail {
            if uri.to_string() == f.case.download_path {
                return f.html(detail.html.clone());
            }
            if uri.to_string() == detail.resolved {
                return TORRENT.into_response();
            }
        } else if uri.to_string() == f.case.download_path {
            return TORRENT.into_response();
        }
        let body = if mode == 1 && !(f.case.id == "aidoruonline" && uri.path() == "/") {
            empty_for(&f.case, &f.body)
        } else {
            f.body.clone()
        };
        let search = uri.path() == f.case.search_path;
        let mut response = if f.case.json && search && [0, 1].contains(&mode) {
            ([("content-type", "application/json")], body).into_response()
        } else {
            f.html(body)
        };
        if cookie == SEED {
            response.headers_mut().insert(
                "set-cookie",
                "session=fixture-cookie-renewed; Path=/".parse().unwrap(),
            );
        }
        if mode == 3 {
            response
                .headers_mut()
                .insert("cf-mitigated", "challenge".parse().unwrap());
        }
        response
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let fixture = Arc::new(Fixture {
        case: c.clone(),
        base: base.clone(),
        body: page(&c, true),
        charset,
        mode: AtomicU8::new(0),
        agent_changed: AtomicU8::new(0),
        hits: Default::default(),
    });
    let app = Router::new()
        .fallback(any(respond))
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
    let settings = if c.user_agent {
        json!({"cookie":SEED,"useragent":AGENT})
    } else {
        json!({"cookie":SEED})
    };
    let i = engine
        .put_indexer(
            Indexer {
                kind: "cardigann".into(),
                name: c.id.clone(),
                url: base.clone(),
                request_delay_ms: 0,
                definition: Some(json!({"builtin_id":c.id,"settings":settings})),
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap_or_else(|e| panic!("{} create: {e}", c.id));
    assert!(fixture.take().is_empty());
    if c.id == "hdtime" {
        for invalid in [
            json!(null),
            json!(""),
            json!(" "),
            json!("é"),
            json!("secret\r\nHost: evil"),
            json!(7),
        ] {
            let invalid_source = Indexer {
                kind: "cardigann".into(),
                name: "invalid fixture".into(),
                url: base.clone(),
                definition: Some(
                    json!({"builtin_id":c.id,"settings":{"cookie":SEED,"useragent":invalid}}),
                ),
                ..Default::default()
            };
            assert!(engine.put_indexer(invalid_source, false).await.is_err());
            assert!(fixture.take().is_empty());
        }
    }
    assert!(!builtin::public_definition(&i)
        .unwrap()
        .to_string()
        .contains(SEED));
    assert!(!builtin::public_definition(&i)
        .unwrap()
        .to_string()
        .contains(AGENT));
    let q = query(&c);
    let first = engine.search(vec![i.id.clone()], q.clone()).await;
    assert!(first.errors.is_empty(), "{}: {:?}", c.id, first.errors);
    assert_eq!(first.results.len(), 1, "{}", c.id);
    let r = &first.results[0];
    assert_eq!(
        r.title,
        c.expected_title.as_deref().unwrap_or("Fixture Release"),
        "{}",
        c.id
    );
    assert_eq!(r.size, Some(1 << 30), "{}", c.id);
    assert_eq!((r.seeders, r.peers), (Some(11), Some(15)), "{}", c.id);
    let hits = fixture.take();
    // One positive preflight validates the supplied cookie, unless the definition
    // has no test; later pages trust it (docs/session-validation.md).
    let preflight = usize::from(!c.no_test);
    assert_eq!(hits.len(), preflight + c.request_count, "{}", c.id);
    if c.no_test {
        assert_eq!(hits[0].1, SEED, "{}", c.id);
    } else {
        assert_eq!(hits[0], (c.login_path.clone(), SEED.into()), "{}", c.id);
        assert_eq!(hits[1].1, RENEWED, "{}", c.id);
    }
    let searches: Vec<_> = hits[preflight..]
        .iter()
        .map(|(path, _)| {
            let url = url::Url::parse(&format!("{}{}", base.trim_end_matches('/'), path)).unwrap();
            request_matches(&c, &url);
            url
        })
        .collect();
    let mut actual_paths: Vec<_> = searches.iter().map(|url| url.path().to_owned()).collect();
    actual_paths.sort();
    actual_paths.dedup();
    let mut expected_paths = vec![c.search_path.clone()];
    expected_paths.extend(c.search_paths.clone());
    expected_paths.sort();
    expected_paths.dedup();
    assert_eq!(
        actual_paths, expected_paths,
        "{}: every selected path executed",
        c.id
    );
    let cached = engine.search(vec![i.id.clone()], q.clone()).await;
    assert_eq!(cached.cached, 1, "{}", c.id);
    assert!(fixture.take().is_empty());
    let mut displayed = first.clone();
    rackett::protocol::present_results(
        &mut displayed,
        "fixture-management-key",
        "http://127.0.0.1:9117",
    );
    let text = serde_json::to_string(&displayed).unwrap();
    assert!(!text.contains(SEED) && !text.contains(RENEWED));
    assert!(displayed.results[0]
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
    assert!(hits.iter().all(|(_, cookie)| cookie == RENEWED));
    if let Some(detail) = &c.detail {
        assert!(hits.iter().any(|(u, _)| *u == c.download_path), "{}", c.id);
        if !detail.resolved.starts_with("magnet:") {
            assert_eq!(hits.last().unwrap().0, detail.resolved, "{}", c.id);
        }
    } else {
        assert_eq!(hits.len(), if c.no_test { 1 } else { 2 }, "{}", c.id);
        assert_eq!(hits.last().unwrap().0, c.download_path);
    }
    assert!(engine
        .download(&i.id, r.source_epoch, "https://other.invalid/file.torrent")
        .await
        .is_err());
    assert!(fixture.take().is_empty());
    engine
        .patch_indexer(&i.id, json!({"name":format!("Updated {}",c.id)}))
        .await
        .unwrap();
    assert!(engine
        .download(&i.id, r.source_epoch, &r.link)
        .await
        .is_err());
    assert!(fixture.take().is_empty());
    drop(engine);
    let engine = Arc::new(Engine::new(path).await.unwrap());
    let restarted = engine.search(vec![i.id.clone()], q.clone()).await;
    assert!(
        restarted.errors.is_empty(),
        "{}: {:?}",
        c.id,
        restarted.errors
    );
    assert_eq!(restarted.results.len(), 1);
    let hits = fixture.take();
    assert!(hits.iter().all(|(_, cookie)| cookie == RENEWED));
    // Validation is in memory: the restored session is tested once, then trusted.
    assert_eq!(hits.len(), preflight + c.request_count, "{}: restart", c.id);
    if !c.no_test {
        assert_eq!(hits[0].0, c.login_path, "{}: restart preflight", c.id);
    }
    if c.user_agent {
        let epoch = restarted.results[0].source_epoch;
        fixture.agent_changed.store(1, Ordering::SeqCst);
        engine.patch_indexer(&i.id, json!({"definition":{"builtin_id":c.id,"settings":{"cookie":SEED,"useragent":AGENT_CHANGED}}})).await.unwrap();
        assert!(engine.download(&i.id, epoch, &r.link).await.is_err());
        assert!(fixture.take().is_empty());
        let changed = engine.search(vec![i.id.clone()], q.clone()).await;
        assert!(changed.errors.is_empty(), "{}: {:?}", c.id, changed.errors);
        assert_eq!(changed.results.len(), 1);
        let hits = fixture.take();
        assert_eq!(
            hits[0].1, SEED,
            "{}: changed agent must isolate renewed cookies",
            c.id
        );
        assert_eq!(hits.len(), preflight + c.request_count, "{}", c.id);
        assert_eq!(hits.last().unwrap().1, RENEWED);
    }
    // Mode 5 redirects searches: without a test it is the only expiry signal.
    let modes: &[u8] = if c.no_test {
        &[1, 2, 3, 4, 5]
    } else {
        &[1, 2, 3, 4]
    };
    for &mode in modes {
        engine.clear_cache().await;
        fixture.mode.store(mode, Ordering::SeqCst);
        let result = engine.search(vec![i.id.clone()], q.clone()).await;
        assert!(result.results.is_empty(), "{} mode {mode}", c.id);
        // Without a test a marker-less HTML page is an empty result, not expiry
        // (pinned CheckIfLoginIsNeeded, `CardigannIndexer.cs:977-980`).
        assert_eq!(
            result.errors.is_empty(),
            mode == 1 || (mode == 2 && c.no_test && !c.json),
            "{} mode {mode}: {:?}",
            c.id,
            result.errors
        );
        let text = serde_json::to_string(&result).unwrap();
        for secret in [SEED, RENEWED, "fixture-private-error"] {
            assert!(!text.contains(secret), "{}", c.id);
        }
        let hits = fixture.take();
        assert!(!hits.is_empty());
        assert!(
            hits.iter().all(|(u, _)| u != "/fixture-login"),
            "{}: search redirect followed",
            c.id
        );
        if !c.no_test {
            // Mode 1 pages trust the validated cookie. Mode 2's expired page clears
            // that trust, so later searches send only their (failing) preflight.
            match mode {
                1 => assert_eq!(hits.len(), c.request_count, "{} mode 1", c.id),
                2 => assert_ne!(hits[0].0, c.login_path, "{} mode 2: preflight", c.id),
                _ => assert!(
                    hits.iter().all(|(u, _)| *u == c.login_path),
                    "{} mode {mode}: {hits:?}",
                    c.id
                ),
            }
        }
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
    assert_eq!(
        fixture.take().len(),
        preflight + c.request_count,
        "{}",
        c.id
    );
    // Stays bound until the test binary exits, so a dropped engine's late request
    // cannot reach a later case's fixture on a reused port.
    drop(server);
}

#[test]
fn source_settings_keep_query_filters_row_filters_and_seedbox_downloads_distinct() {
    for c in cases() {
        let root = root(&c);
        let p = Plan::compile(&root).unwrap();
        let now = "2026-10-29T12:00:00Z".parse().unwrap();
        let settings = BTreeMap::from([("freeleech".into(), json!(true))]);
        let ctx = p.context(&settings, &query(&c), now);
        let requests = p.build_requests(&ctx).unwrap();
        assert_eq!(requests.len(), c.request_count, "{}", c.id);
        let expected = match c.id.as_str() {
            "abtorrents" => Some(("only_free", "1")),
            "aidoruonline" => Some(("fl", "1")),
            "asiandvdclub" => Some(("golden", "1")),
            "cgpeers" | "crt2fa" | "empornium2fa" | "happyfappy2fa" | "kufirc2fa"
            | "pixelcove2fa" => Some(("filter_freeleech", "1")),
            "cspt" | "hqmusic" => Some(("spstate", "2")),
            "docspedia" => Some(("free", "1")),
            "exitorrent-org" => Some(("polish", "3")),
            "herobit" => Some(("type", "3")),
            "mma-torrents" => Some(("freeleech", "2")),
            "pwtorrents2fa" | "xwtclassics2fa" => Some(("incldead", "3")),
            "torrenting" => Some(("free", "on")),
            _ => None,
        };
        for (n, request) in requests.iter().enumerate() {
            let url = built_url(&c, &p, request);
            request_matches(&c, &url);
            if let Some((key, value)) = expected {
                assert!(
                    url.query_pairs().any(|(k, v)| k == key && v == value),
                    "{}: freeleech query",
                    c.id
                );
            }
            if c.id == "learnflakes" {
                assert!(url
                    .query_pairs()
                    .any(|(k, v)| k == "page" && v == (n + 1).to_string()));
            }
            assert_eq!(
                extract(&p, request, &page(&c, true), &ctx).unwrap().len(),
                1,
                "{}",
                c.id
            );
            if root
                .get("search")
                .unwrap()
                .get("rows")
                .unwrap()
                .string("selector")
                .unwrap()
                .contains(".Config.freeleech")
            {
                assert!(
                    extract(&p, request, &page(&c, false), &ctx)
                        .unwrap()
                        .is_empty(),
                    "{}: freeleech row filter",
                    c.id
                );
            }
        }
        if matches!(c.id.as_str(), "pwtorrents2fa" | "xwtclassics2fa") {
            let ctx = p.context(
                &BTreeMap::from([("seedbox".into(), json!(true))]),
                &query(&c),
                now,
            );
            let request = p.build_requests(&ctx).unwrap().remove(0);
            let rows = extract(&p, &request, &page(&c, false), &ctx).unwrap();
            let path = if c.id == "pwtorrents2fa" {
                "/seeddownload.php?id=42"
            } else {
                "/download2.php?id=42"
            };
            assert!(rows[0].link.ends_with(path), "{}: seedbox", c.id);
        }
    }
}

#[test]
fn browser_cookie_layouts_keep_provider_specific_fields_and_alternative_branches() {
    let now = "2026-10-29T12:00:00Z".parse().unwrap();
    for c in cases().into_iter().filter(|c| c.user_agent) {
        let p = Plan::compile(&root(&c)).unwrap();
        let run = |body: String, settings: BTreeMap<String, serde_json::Value>| {
            let ctx = p.context(&settings, &query(&c), now);
            let request = p.build_requests(&ctx).unwrap().remove(0);
            let rows =
                extract(&p, &request, &body, &ctx).unwrap_or_else(|e| panic!("{}: {e}", c.id));
            assert_eq!(rows.len(), 1, "{}", c.id);
            rows.into_iter().next().unwrap()
        };
        let normal = page(&c, false);
        let r = run(normal.clone(), BTreeMap::new());
        match c.id.as_str() {
            "hdtime" | "ptskit" => {
                assert_eq!(r.attributes["imdbid"], "1234567", "{}", c.id);
                let fallback = normal.replace(" title=\"Fixture Release\"", "");
                assert_eq!(run(fallback, BTreeMap::new()).title, "Fixture Release");
                let direct_date = normal.replace(
                    "<span title=\"2026-10-28 18:30:00\">date</span>",
                    "2026-10-2818:30:00",
                );
                assert_eq!(run(direct_date, BTreeMap::new()).published, r.published);
                let silver = page(&c, true).replace("pro_free2up", "pro_50pctdown2up");
                let silver = run(silver, BTreeMap::new());
                assert_eq!(silver.attributes["downloadvolumefactor"], "0.5");
                assert_eq!(silver.attributes["uploadvolumefactor"], "2");
                let missing = normal
                    .replace(">1 GB<", "><")
                    .replace(">11<", "><")
                    .replace(">4<", "><");
                let missing = run(missing, BTreeMap::new());
                assert_eq!(missing.size, Some(512 << 20));
                assert_eq!((missing.seeders, missing.peers), (Some(0), Some(0)));
            }
            "pterclub" => {
                let alternate =
                    normal.replace("comment.php?id=42", "details.php?id=42#startcomments");
                let alternate = run(alternate, BTreeMap::new());
                assert_eq!(
                    (
                        alternate.size,
                        alternate.seeders,
                        alternate.peers,
                        alternate.published
                    ),
                    (r.size, r.seeders, r.peers, r.published)
                );
                assert_eq!(
                    run(
                        normal.replace("details.php?id=42", "detailsgame.php?id=42"),
                        BTreeMap::new()
                    )
                    .title,
                    r.title
                );
                assert!(r.attributes["description"].contains("Description"));
            }
            "animebybelka" => {
                let changed = normal.replace("Fixture Release", "Тест Fixture Release");
                let result = run(
                    changed,
                    BTreeMap::from([
                        ("stripcyrillic".into(), json!(true)),
                        ("addrussiantotitle".into(), json!(true)),
                    ]),
                );
                assert_eq!(result.title, " Fixture Release RUS");
                assert_eq!(
                    run(
                        normal.replace("28 окт 2026, 13:30", "Сегодня 10:30"),
                        BTreeMap::new()
                    )
                    .published
                    .as_deref(),
                    Some("2026-10-29T10:30:00Z")
                );
                assert_eq!(
                    run(
                        normal.replace("28 окт 2026, 13:30", "Вчера 10:30"),
                        BTreeMap::new()
                    )
                    .published
                    .as_deref(),
                    Some("2026-10-28T10:30:00Z")
                );
            }
            "huntorrent" => {
                assert_eq!(r.attributes["minimumseedtime"], "115200");
                assert!(r.attributes["description"].contains("Action"));
            }
            "korsar" => {
                // Preserve the pinned definition's reversed Today/Yesterday replacements.
                assert_eq!(
                    run(
                        normal.replace("28 окт 2026, 13:30", "Вчера, 10:30"),
                        BTreeMap::new()
                    )
                    .published
                    .as_deref(),
                    Some("2026-10-29T10:30:00Z")
                );
                assert_eq!(
                    run(
                        normal.replace("28 окт 2026, 13:30", "Сегодня, 10:30"),
                        BTreeMap::new()
                    )
                    .published
                    .as_deref(),
                    Some("2026-10-28T10:30:00Z")
                );
                assert_eq!(
                    run(
                        normal.replace("28 окт 2026, 13:30", "2 часа назад"),
                        BTreeMap::new()
                    )
                    .published
                    .as_deref(),
                    Some("2026-10-29T10:00:00Z")
                );
            }
            "majomparade" => {
                assert!(r.attributes["description"].contains("Verified: Action"));
                for (markup, factor) in [("0.5x", "0.5"), ("3x", "3"), ("4x", "4")] {
                    assert_eq!(
                        run(
                            page(&c, true).replace(">2x<", &format!(">{markup}<")),
                            BTreeMap::new()
                        )
                        .attributes["uploadvolumefactor"],
                        factor
                    );
                }
                assert!(run(
                    normal.replace("torrent-chip--mod-ok", "torrent-chip--mod-unknown"),
                    BTreeMap::new()
                )
                .attributes["description"]
                    .contains("Unverified"));
            }
            "matrix" => {
                assert_eq!(r.attributes["description"], "Verified - Action");
                assert!(r.attributes["poster"].ends_with("/poster.jpg"));
                assert!(run(
                    normal.replace("torrent-ok", "torrent-error"),
                    BTreeMap::new()
                )
                .attributes["description"]
                    .contains("Faulty Upload"));
            }
            "teamhd" => {
                assert_eq!(
                    run(
                        page(&c, true).replace("#f2b101", "#828b8b"),
                        BTreeMap::new()
                    )
                    .attributes["downloadvolumefactor"],
                    "0.5"
                );
                assert_eq!(
                    run(
                        normal.replace(
                            "</body>",
                            "<div class=\"justify-content-center\">Включен фрилич до</div></body>"
                        ),
                        BTreeMap::new()
                    )
                    .attributes["downloadvolumefactor"],
                    "0"
                );
                assert_eq!(
                    run(
                        normal.replace("Fixture Release", "Fixture Release [Season 3]"),
                        BTreeMap::new()
                    )
                    .title,
                    "Fixture Release S03"
                );
            }
            "torrentmasters" => {
                assert!(r.attributes["description"].contains("Local title<br>Action"));
                assert!(r.attributes["poster"].ends_with("/poster.jpg"));
                let boosted =
                    normal.replace("1 GB", "1 GB <small>2</small><i class=\"fa-arrow-up\"></i>");
                let boosted = run(boosted, BTreeMap::new());
                assert_eq!(boosted.attributes["uploadvolumefactor"], "2");
            }
            "turktorrent" => {
                assert_eq!(
                    run(
                        page(&c, true).replace("freedownload.gif", "silverdownload.gif"),
                        BTreeMap::new()
                    )
                    .attributes["downloadvolumefactor"],
                    "0.5"
                );
                assert_eq!(
                    run(
                        normal.replace("Fixture Release", "Fixture Release 28.10.2026"),
                        BTreeMap::new()
                    )
                    .title,
                    "Fixture Release 2026.10.28"
                );
            }
            "ex-torrenty" => {
                // MULTi is replaced only with `multilang`, and only without a language
                // tag after it; a standalone PL becomes POLISH.
                let multi = normal.replace("Fixture Release", "Fixture Release MULTi");
                assert_eq!(
                    run(multi.clone(), BTreeMap::new()).title,
                    "Fixture Release MULTi"
                );
                let multilang = BTreeMap::from([("multilang".into(), json!(true))]);
                assert_eq!(
                    run(multi.clone(), multilang.clone()).title,
                    "Fixture Release POLISH"
                );
                assert_eq!(
                    run(
                        multi.clone(),
                        BTreeMap::from([
                            ("multilang".into(), json!(true)),
                            ("multilanguage".into(), json!("MULTi POLISH")),
                        ])
                    )
                    .title,
                    "Fixture Release MULTi POLISH"
                );
                assert_eq!(
                    run(
                        normal.replace("Fixture Release", "Fixture Release MULTi PL"),
                        multilang
                    )
                    .title,
                    "Fixture Release MULTi POLISH"
                );
                // The ordered category case: "Filmy HD" is site category 17 (Movies/HD).
                let hd = run(normal.replace(">Sport<", ">Filmy HD<"), BTreeMap::new());
                assert!(hd.categories.contains(&2040), "{:?}", hd.categories);
            }
            "gaytorrentru" => {
                // The download link is built from the id and the URL-encoded title.
                let ampersand = run(
                    normal.replace(">Fixture Release<", ">Fixture &amp; Release<"),
                    BTreeMap::new(),
                );
                assert!(
                    ampersand.link.ends_with(
                        "/download.php?id=42&n=Fixture+%26+Release.torrent&p=dodownload"
                    ),
                    "{}",
                    ampersand.link
                );
                // Seeder and leecher links are optional and default to zero.
                let missing = normal.replace(">11<", "><").replace(">4<", "><");
                let missing = run(missing, BTreeMap::new());
                assert_eq!((missing.seeders, missing.peers), (Some(0), Some(0)));
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn browser_cookie_queries_keep_category_routes_ids_sort_and_freeleech() {
    let now = "2026-10-29T12:00:00Z".parse().unwrap();
    for c in cases().into_iter().filter(|c| c.user_agent) {
        let p = Plan::compile(&root(&c)).unwrap();
        let settings = BTreeMap::from([
            ("useragent".into(), json!(AGENT)),
            ("freeleech".into(), json!(true)),
        ]);
        let requests = p
            .build_requests(&p.context(&settings, &query(&c), now))
            .unwrap();
        assert_eq!(requests.len(), c.request_count, "{}", c.id);
        for r in requests {
            let mut expected = vec![("User-Agent".to_owned(), AGENT.to_owned())];
            if let Some(suffix) = &c.referer {
                // The plan renders the definition's first link as its site link.
                let link = root(&c).get("links").unwrap().items().unwrap()[0]
                    .text()
                    .unwrap()
                    .to_owned();
                expected.push(("Referer".into(), format!("{link}{suffix}")));
            }
            let mut headers = r.headers.clone();
            headers.sort();
            expected.sort();
            assert_eq!(headers, expected, "{}", c.id);
            request_matches(&c, &url::Url::parse(&r.url).unwrap());
            let key = match c.id.as_str() {
                "hdtime" | "ptskit" | "pterclub" => Some(("spstate", "2")),
                "matrix" => Some(("incldead", "3")),
                "teamhd" => Some(("free", "1")),
                "gaytorrentru" => Some(("infl", "1")),
                _ => None,
            };
            if let Some((key, value)) = key {
                assert!(url::Url::parse(&r.url)
                    .unwrap()
                    .query_pairs()
                    .any(|(k, v)| k == key && v == value));
            }
        }
        if matches!(c.id.as_str(), "ptskit" | "huntorrent" | "majomparade") {
            let mut q = query(&c);
            q.params.insert("cat".into(), "6000".into());
            let req = p.build_requests(&p.context(&settings, &q, now)).unwrap();
            assert_eq!(req.len(), 1, "{}", c.id);
            let expected = match c.id.as_str() {
                "ptskit" => "/special.php",
                "huntorrent" => "/browse/xxx",
                _ => "/torrents/xxx/",
            };
            assert_eq!(url::Url::parse(&req[0].url).unwrap().path(), expected);
        }
        if matches!(c.id.as_str(), "hdtime" | "ptskit" | "pterclub") {
            for (id, key, area) in [
                ("imdbid", "tt1234567", "4"),
                (
                    "doubanid",
                    "123456",
                    if c.id == "pterclub" { "5" } else { "1" },
                ),
            ] {
                let mut q = query(&c);
                q.params.insert(id.into(), key.into());
                let req = p.build_requests(&p.context(&settings, &q, now)).unwrap();
                let url = url::Url::parse(&req[0].url).unwrap();
                assert!(
                    url.query_pairs().any(|(k, v)| k == "search" && v == key),
                    "{} {id}",
                    c.id
                );
                assert!(
                    url.query_pairs()
                        .any(|(k, v)| k == "search_area" && v == area),
                    "{} {id}",
                    c.id
                );
            }
        }
    }
}
