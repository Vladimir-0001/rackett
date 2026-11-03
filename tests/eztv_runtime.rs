//! Loopback readiness fixtures for the unchanged pinned EZTV definition
//! (Jackett fe8876d2 `src/Jackett.Common/Definitions/eztv.yml`). The runtime is
//! assembled from public `builtin::Runtime` fields and `Plan::compile`, as the
//! runtime unit fixtures do, plus an Engine create/restart/cookie-origin lifecycle.
//! Every HTML body below is
//! SYNTHETIC and derived only from the definition's selectors, filters and
//! comments; none is a capture of a live page. No credentials, no remote traffic.
//! Upstream row-error policy: `CardigannIndexer.cs:1952-1955` logs and skips a
//! failing row; path encoding: `CardigannIndexer.cs:1542-1543`.
use chrono::{Duration as Age, TimeZone, Utc};
use rackett::{
    builtin::{self, Runtime},
    cardigann::{self, parse, Limits},
    cardigann_exec::Plan,
    engine::Engine,
    models::{Indexer, Release, SearchQuery, SearchResponse},
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{atomic::AtomicUsize, atomic::Ordering, Arc, Mutex},
    time::Instant,
};

const SOURCE: &str = include_str!("../vendor/jackett/definitions/eztv.yml");
const SHA256: &str = "f277bf623a48c987981c1cd21f0d0dbca4e9aee9e564e6126b4dccf40360bcaf";
/// `search.headers.cookie`, verbatim.
const COOKIE: &str = "sort_no=100; q_filter=all; q_filter_web=on; q_filter_reality=on; q_filter_x265=on; layout=def_wlinks";
const MAGNET_A: &str = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Show+Name";
const MAGNET_B: &str = "magnet:?xt=urn:btih:89abcdef0123456789abcdef0123456789abcdef&dn=Other";
const MAGNET_C: &str = "magnet:?xt=urn:btih:fedcba9876543210fedcba9876543210fedcba98&dn=Third";
/// A download mirror on a foreign origin, as the `def_wlinks` layout lists beside magnets.
const MIRROR: &str = "https://mirror.example/torrent/show.name.s01e03.torrent";
const PRIVATE: &str = "fixture private upstream body";

fn indexer(url: &str) -> Indexer {
    Indexer {
        id: "eztv-fixture".into(),
        name: "EZTV fixture".into(),
        kind: "cardigann".into(),
        url: url.into(),
        request_delay_ms: 0,
        definition: Some(json!({"builtin_id":"eztv","settings":{}})),
        ..Default::default()
    }
}
/// The pinned plan with its links replaced by `base`, mirroring `Runtime::compile`.
fn plan(base: &str) -> Plan {
    let mut root = parse(SOURCE, Limits::default()).unwrap();
    if let cardigann::Value::Mapping(pairs) = &mut root.value {
        for (k, v) in pairs {
            if k.text() == Some("links") {
                let mut link = v.items().unwrap()[0].clone();
                if let cardigann::Value::Scalar(s) = &mut link.value {
                    s.text = base.into();
                }
                v.value = cardigann::Value::Sequence(vec![link]);
            }
        }
    }
    let plan = Plan::compile(&root).unwrap();
    assert!(plan.blockers().is_empty(), "{:?}", plan.blockers().len());
    plan
}
/// `container` is `None` exactly as `Runtime::compile` would leave it for EZTV.
fn runtime(base: &str, container: Option<&str>) -> Runtime {
    Runtime {
        plan: Arc::new(plan(base)),
        settings: BTreeMap::new(),
        delay_ms: 0,
        modes: builtin::source_modes(&indexer(base)),
        empty_html_container: container.map(Into::into),
        download: None,
        api_origin: None,
    }
}
fn query(params: &[(&str, &str)]) -> SearchQuery {
    SearchQuery {
        params: params
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect(),
    }
}

struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: String,
}
fn ok(body: String) -> Reply {
    Reply {
        status: 200,
        headers: vec![("content-type", "text/html; charset=utf-8".into())],
        body,
    }
}
type Respond = Arc<dyn Fn(&str) -> Reply + Send + Sync>;
/// Path-and-query plus every Cookie header value of each request.
type Seen = Arc<Mutex<Vec<(String, Vec<String>)>>>;
struct Server {
    url: String,
    seen: Seen,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    fn uris(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.0.clone())
            .collect()
    }
}
async fn serve(respond: Respond) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let seen = Seen::default();
    let captured = seen.clone();
    let app = axum::Router::new().fallback(
        move |uri: axum::http::Uri, headers: axum::http::HeaderMap| {
            let (respond, captured) = (respond.clone(), captured.clone());
            async move {
                let target = uri.path_and_query().unwrap().to_string();
                let cookies = headers
                    .get_all(axum::http::header::COOKIE)
                    .iter()
                    .map(|v| v.to_str().unwrap().to_owned())
                    .collect();
                captured.lock().unwrap().push((target.clone(), cookies));
                let reply = respond(&target);
                let mut response = axum::response::IntoResponse::into_response(reply.body);
                *response.status_mut() = axum::http::StatusCode::from_u16(reply.status).unwrap();
                response.headers_mut().clear();
                for (k, v) in reply.headers {
                    response.headers_mut().append(k, v.parse().unwrap());
                }
                response
            }
        },
    );
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server { url, seen, task }
}
/// Bundled-source redirect/retry/cookie policy, with a timeout for this fixture client.
async fn search(rt: &Runtime, url: &str, q: &SearchQuery) -> anyhow::Result<Vec<Release>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .unwrap();
    rackett::builtin_runtime::search(rt, &client, &indexer(url), q, &mut Instant::now()).await
}

// Synthetic markup shaped by the definition: `tr[name='hover'].forum_header_border`
// rows inside `table.forum_header_border`, with title/details in cell 2, links in
// cell 3, size in 4, age in 5 and seeders in 6.
fn row(href: &str, title: &str, links: &str, size: &str, age: &str, seeders: &str) -> String {
    format!(
        "<tr name=\"hover\" class=\"forum_header_border\">\
         <td class=\"forum_thread_post\"><a href=\"/shows/1/show/\" title=\"Show Torrents\"><img src=\"/s.png\"></a></td>\
         <td class=\"forum_thread_post\"><a href=\"{href}\" title=\"{title}\" class=\"epinfo\">listing text</a></td>\
         <td class=\"forum_thread_post\">{links}</td>\
         <td class=\"forum_thread_post\">{size}</td>\
         <td class=\"forum_thread_post\">{age}</td>\
         <td class=\"forum_thread_post\"><font color=\"green\">{seeders}</font></td></tr>"
    )
}
fn magnet(href: &str) -> String {
    format!("<a href=\"{href}\" class=\"magnet\" title=\"Magnet Link\"></a>")
}
fn mirror(href: &str) -> String {
    format!("<a href=\"{href}\" class=\"download_1\" rel=\"nofollow\"></a>")
}
fn table(heading: &str, rows: &str) -> String {
    format!(
        "<table class=\"forum_header_border\" width=\"100%\"><tr><td class=\"section_post_header\" colspan=\"6\"><h1>{heading}</h1></td></tr>{rows}</table>"
    )
}
fn page(body: &str) -> String {
    format!("<!DOCTYPE html><html><head><title>EZTV</title></head><body>{body}</body></html>")
}
fn good_row() -> String {
    row(
        "/ep/1/show-name-s01e02/",
        "Show Name S01E02 720p WEB H264-GROUP [eztv] (2.5 GB)",
        &(magnet(MAGNET_A) + &mirror(MIRROR)),
        "2.5 GB",
        "3h 54m",
        "57",
    )
}
/// Keywordless layout: one table mentioning both `Latest` and `Releases` (the
/// selector list must not duplicate it) plus a non-matching table that also has
/// hover rows with magnets.
fn home() -> String {
    let rows = [
        good_row(),
        // No a.magnet anywhere in the row: excluded by `:has(a.magnet)`.
        row(
            "/ep/2/no-magnet/",
            "No Magnet S01E01 [eztv]",
            &mirror(MIRROR),
            "1 GB",
            "1h",
            "3",
        ),
        // Mirror precedes the magnet: the download selector list yields document order.
        row(
            "/ep/3/show-name-s01e03/",
            "Show Name S01E03 1080p [eztv] (3 GB)",
            &(mirror(MIRROR) + &magnet(MAGNET_B)),
            "3 GB",
            "2d 1h",
            "8",
        ),
        // Empty size cell: optional field falls back to `512 MB`.
        row(
            "/ep/4/other-show/",
            "Other Show S02E03 1080p [eztv]",
            &magnet(MAGNET_C),
            "",
            "15m",
            "0",
        ),
        // Not a hover row.
        "<tr class=\"forum_header_border\"><td></td><td><a href=\"/ep/5/x/\" title=\"Header Row\">x</a></td><td><a class=\"magnet\" href=\"magnet:?xt=urn:btih:0000000000000000000000000000000000000000\"></a></td></tr>".into(),
    ]
    .concat();
    let popular = row(
        "/ep/9/popular/",
        "Popular Show S09E09 [eztv] (1 GB)",
        &magnet(MAGNET_A),
        "1 GB",
        "1h",
        "999",
    );
    page(&(table("Popular Shows", &popular) + &table("<b>EZTV</b> Latest Releases", &rows)))
}

#[tokio::test]
async fn pinned_definition_create_search_restart_and_origin_cookie_reset() {
    use sha2::{Digest, Sha256};
    assert_eq!(format!("{:x}", Sha256::digest(SOURCE.as_bytes())), SHA256);
    assert!(builtin::CHECKSUMS.contains(&format!("{SHA256}  definitions/eztv.yml")));
    let embedded = builtin::SOURCES
        .iter()
        .find(|(id, _)| *id == "eztv")
        .unwrap();
    assert_eq!(embedded.1, SOURCE);
    assert!(builtin::READY_IDS.contains(&"eztv"));
    let entry = builtin::catalog()
        .iter()
        .find(|v| v["id"] == "eztv")
        .unwrap();
    assert_eq!(entry["ready"], true);
    assert!(entry["blocked_reason"].is_null());
    assert_eq!(entry["caps"]["search"], json!(["q"]));
    assert_eq!(entry["caps"]["tv-search"], json!(["q", "season", "ep"]));
    // The interpreter itself has no blockers and maps category 1 to TV.
    assert_eq!(plan("https://eztvx.to/").supported_categories(), vec![5000]);
    let server = serve(Arc::new(|_| ok(home()))).await;
    let mut stock = indexer(&server.url);
    stock.cookie = "session=fixture-private".into();
    assert!(Runtime::compile(&stock).is_ok());
    assert!(rackett::adapters::validate(&stock).is_ok());
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    let engine = Arc::new(Engine::new(config.clone()).await.unwrap());
    let saved = engine.put_indexer(stock, false).await.unwrap();
    let result = engine
        .search(vec![saved.id.clone()], query(&[("q", "Show Name")]))
        .await;
    assert!(result.errors.is_empty());
    assert!(!result.results.is_empty());
    drop(engine);
    let engine = Arc::new(Engine::new(config).await.unwrap());
    assert_eq!(
        engine.indexer(&saved.id).await.unwrap().cookie,
        "session=fixture-private"
    );
    let result = engine
        .search(vec![saved.id.clone()], query(&[("q", "Show Name")]))
        .await;
    assert!(result.errors.is_empty());
    assert!(!result.results.is_empty());
    assert!(server
        .seen
        .lock()
        .unwrap()
        .iter()
        .all(|(_, cookies)| cookies.len() == 1
            && cookies[0].contains("session=fixture-private")
            && cookies[0].contains("layout=def_wlinks")));
    let other = serve(Arc::new(|_| ok(home()))).await;
    let changed = engine
        .patch_indexer(&saved.id, json!({"url":other.url}))
        .await
        .unwrap();
    assert!(changed.cookie.is_empty());
    let result = engine
        .search(vec![saved.id], query(&[("q", "Show Name")]))
        .await;
    assert!(result.errors.is_empty());
    assert!(other
        .seen
        .lock()
        .unwrap()
        .iter()
        .all(|(_, cookies)| cookies.len() == 1 && !cookies[0].contains("fixture-private")));
}

#[tokio::test]
async fn keyword_filters_choose_the_path_and_only_the_fixed_cookie_is_sent() {
    // A Set-Cookie on every response must never come back: there is no cookie jar.
    let server = serve(Arc::new(|_: &str| {
        let mut reply = ok(page(&table("Releases", "")));
        reply
            .headers
            .push(("set-cookie", "session=fixture-session; Path=/".into()));
        reply
    }))
    .await;
    let rt = runtime(&server.url, None);
    let cases: &[(&[(&str, &str)], &str)] = &[
        (&[], "/home"),
        (&[("q", "Show Name")], "/search/Show-Name"),
        // `\bS\d{2,3}\b` removes a season-only tag, two or three digits.
        (&[("q", "Show Name S01")], "/search/Show-Name"),
        (&[("q", "Show Name S001")], "/search/Show-Name"),
        (&[("q", "S01 Show Name")], "/search/Show-Name"),
        (&[("q", "Show Name S01E02")], "/search/Show-Name-S01E02"),
        (&[("q", "Show Name S1")], "/search/Show-Name-S1"),
        (&[("q", "Show Name S1000")], "/search/Show-Name-S1000"),
        // A season-only query leaves no keywords and falls back to the home page.
        (&[("q", "S01")], "/home"),
        (
            &[("t", "tvsearch"), ("q", "Show Name"), ("season", "1")],
            "/search/Show-Name",
        ),
        (
            &[
                ("t", "tvsearch"),
                ("q", "Show Name"),
                ("season", "1"),
                ("ep", "2"),
            ],
            "/search/Show-Name-S01E02",
        ),
        // `-` removed, spaces become `-`, `&` removed (in that order).
        (&[("q", "Law & Order - SVU")], "/search/Law--Order--SVU"),
        (&[("q", "Spider-Man")], "/search/SpiderMan"),
        (&[("q", "  Show   Name  ")], "/search/Show---Name"),
        // WebUtility.UrlEncode with `+` rewritten to `%20`.
        (&[("q", "Café/50%")], "/search/Caf%C3%A9%2F50%25"),
    ];
    for (params, expected) in cases {
        let before = server.uris().len();
        let rows = search(&rt, &server.url, &query(params)).await.unwrap();
        assert!(rows.is_empty());
        assert_eq!(
            server.uris()[before..],
            [expected.to_string()],
            "{params:?}"
        );
    }
    for (_, cookies) in server.seen.lock().unwrap().iter() {
        assert_eq!(cookies, &[COOKIE.to_string()]);
    }
    // Unsupported parameters fail before any request.
    let before = server.uris().len();
    assert!(search(&rt, &server.url, &query(&[("imdbid", "tt1")]))
        .await
        .is_err());
    assert!(search(&rt, &server.url, &query(&[("t", "movie")]))
        .await
        .is_err());
    assert_eq!(server.uris().len(), before);
}

/// A runtime result: `search` validates the extracted magnet and promotes it to `link`.
fn assert_good_row(r: &Release, url: &str) {
    assert_eq!(r.link, MAGNET_A);
    assert_good_fields(r, url);
}
fn assert_good_fields(r: &Release, url: &str) {
    assert_eq!(r.attributes["magnet"], MAGNET_A);
    assert_eq!(r.title, "Show Name S01E02 720p WEB H264-GROUP");
    assert_eq!(
        r.details.as_deref(),
        Some(&*format!("{url}ep/1/show-name-s01e02/"))
    );
    assert_eq!(r.size, Some(2_684_354_560));
    assert_eq!(r.seeders, Some(57));
    assert_eq!(r.peers, Some(57));
    assert!(r.categories.contains(&5000), "{:?}", r.categories);
    assert_eq!(r.attributes["downloadvolumefactor"], "0");
    assert_eq!(r.attributes["uploadvolumefactor"], "1");
}

#[test]
fn source_derived_fields_with_a_fixed_clock() {
    let base = "https://eztvx.to/";
    let plan = plan(base);
    let now = Utc.with_ymd_and_hms(2026, 10, 27, 12, 0, 0).unwrap();
    let ctx = plan.context(&BTreeMap::new(), &SearchQuery::default(), now);
    let request = plan.build_requests(&ctx).unwrap().remove(0);
    assert_eq!(request.url, format!("{base}home"));
    let rows = plan.extract(&request, &home(), &ctx).unwrap();
    let titles: Vec<_> = rows.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(
        titles,
        [
            "Show Name S01E02 720p WEB H264-GROUP",
            "Show Name S01E03 1080p",
            "Other Show S02E03 1080p",
        ]
    );
    // Offline extraction keeps a magnet download as an attribute with an empty link.
    assert_eq!(rows[0].link, "");
    assert_good_fields(&rows[0], base);
    // `td:nth-child(5)` + " ago": "3h 54m ago" is relative to the request clock.
    let published = |age: Age| Some((now - age).to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
    assert_eq!(rows[0].published, published(Age::minutes(3 * 60 + 54)));
    assert_eq!(rows[1].published, published(Age::hours(49)));
    assert_eq!(rows[2].published, published(Age::minutes(15)));
    // Document order inside cell 3 wins over the order of the selector list.
    assert_eq!(rows[1].link, MIRROR);
    assert_eq!(rows[1].size, Some(3 * 1024 * 1024 * 1024));
    assert_eq!(rows[2].size, Some(512 * 1024 * 1024));
    assert_eq!(rows[2].seeders, Some(0));
    // `\(.*\)$` is greedy from the first parenthesis, as in .NET.
    let body = page(&table(
        "Latest",
        &row(
            "/ep/6/x/",
            "Show (US) S01E01 HDTV x264-GRP [eztv] (350.5 MB)",
            &magnet(MAGNET_A),
            "350.5 MB",
            "1h",
            "1",
        ),
    ));
    assert_eq!(
        plan.extract(&request, &body, &ctx).unwrap()[0].title,
        "Show"
    );
}

#[tokio::test]
async fn loopback_latest_and_releases_tables_andmatch_and_presentation() {
    let server = serve(Arc::new(|target: &str| {
        if target == "/home" {
            ok(home())
        } else {
            // Search layout: rows only in the `Releases` table; andmatch filters titles.
            let rows = good_row()
                + &row(
                    "/ep/7/different/",
                    "Different Program S01E02 [eztv] (1 GB)",
                    &magnet(MAGNET_B),
                    "1 GB",
                    "1h",
                    "4",
                );
            let other = row(
                "/ep/8/x/",
                "Show Name S01E02 Sidebar [eztv]",
                &magnet(MAGNET_C),
                "1 GB",
                "1h",
                "4",
            );
            ok(page(
                &(table("Releases", &rows) + &table("Trending", &other)),
            ))
        }
    }))
    .await;
    let rt = runtime(&server.url, None);
    let before = Utc::now();
    let rows = search(&rt, &server.url, &SearchQuery::default())
        .await
        .unwrap();
    let after = Utc::now();
    assert_eq!(rows.len(), 3);
    assert_good_row(&rows[0], &server.url);
    assert!(rows
        .iter()
        .all(|r| r.indexer_id == "eztv-fixture" && r.indexer == "EZTV fixture"));
    // Bounded clock check: the runtime uses the wall clock at request time.
    let published = chrono::DateTime::parse_from_rfc3339(rows[0].published.as_deref().unwrap())
        .unwrap()
        .with_timezone(&Utc);
    let age = Age::minutes(3 * 60 + 54);
    assert!(published >= before - age - Age::seconds(2), "{published}");
    assert!(published <= after - age + Age::seconds(2), "{published}");

    let found = search(&rt, &server.url, &query(&[("q", "Show Name")]))
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_good_row(&found[0], &server.url);
    assert_eq!(server.uris(), ["/home", "/search/Show-Name"]);

    // A mirror-first row loses its magnet, then becomes an opaque local ticket.
    // Even after activation, Engine::download rejects this foreign origin.
    // Resolve this limitation before activation; presentation alone is not download proof.
    assert!(!rows[1].attributes.contains_key("magnet"));
    let mut response = SearchResponse {
        results: rows,
        ..Default::default()
    };
    let links = rackett::protocol::Links::new(
        std::sync::Arc::new(rackett::ticket::TicketKey::from_bytes(&[1; 32])),
        Default::default(),
        "localkey",
        "http://127.0.0.1:9117",
    );
    rackett::protocol::present_results(&mut response, &links);
    let results = &response.results;
    assert_eq!(results[0].link, MAGNET_A);
    assert!(results[1].link.starts_with("http://127.0.0.1:9117/dl/"));
    assert!(!results[1].link.contains("mirror.example"));
    assert!(results[0].guid.starts_with("urn:rackett:"));
    assert_eq!(results[2].link, MAGNET_C);
}

/// Status, response headers and the expected locally defined diagnostic.
type ErrorCase = (u16, &'static [(&'static str, &'static str)], &'static str);
#[tokio::test]
async fn http_errors_and_challenges_are_sanitized_and_not_retried() {
    let cases: [ErrorCase; 9] = [
        (403, &[], "HTTP 403 Forbidden: Access denied"),
        (429, &[], "HTTP 429 Too Many Requests: Rate limit exceeded"),
        (
            500,
            &[],
            "HTTP 500 Internal Server Error: The source is unavailable",
        ),
        (502, &[], "HTTP 502 Bad Gateway: The source is unavailable"),
        (
            503,
            &[],
            "HTTP 503 Service Unavailable: The source is unavailable",
        ),
        (
            200,
            &[("cf-mitigated", "challenge")],
            "HTTP 200 OK: Cloudflare challenge detected",
        ),
        (
            403,
            &[("cf-mitigated", "challenge")],
            "HTTP 403 Forbidden: Cloudflare challenge detected",
        ),
        (
            503,
            &[("cf-mitigated", "challenge"), ("server", "cloudflare")],
            "Cloudflare challenge detected",
        ),
        // `Server: cloudflare` alone is not a challenge signal.
        (
            403,
            &[("server", "cloudflare")],
            "HTTP 403 Forbidden: Access denied",
        ),
    ];
    for (status, headers, expected) in cases {
        let headers: Vec<_> = headers.iter().map(|(k, v)| (*k, v.to_string())).collect();
        let server = serve(Arc::new(move |_: &str| Reply {
            status,
            headers: headers.clone(),
            body: page(&format!("{PRIVATE} {}", table("Latest", &good_row()))),
        }))
        .await;
        let rt = runtime(&server.url, None);
        let error = search(&rt, &server.url, &query(&[("q", "private-query")]))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{status}: {error}");
        for leaked in [
            PRIVATE,
            "private-query",
            "privatequery",
            "mirror.example",
            &server.url,
        ] {
            assert!(!error.contains(leaked), "{error}");
        }
        assert_eq!(server.uris(), ["/search/privatequery"], "{status}");
    }
    // An ordinary Cloudflare-fronted 200 is accepted.
    let server = serve(Arc::new(|_: &str| {
        let mut reply = ok(page(&table("Latest", &good_row())));
        reply.headers.push(("server", "cloudflare".into()));
        reply
            .headers
            .push(("cf-ray", "0000000000000000-AMS".into()));
        reply
    }))
    .await;
    let rows = search(
        &runtime(&server.url, None),
        &server.url,
        &SearchQuery::default(),
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_good_row(&rows[0], &server.url);
}

#[tokio::test]
async fn redirects_stay_on_origin_and_are_bounded() {
    let server = serve(Arc::new(|target: &str| match target {
        "/home" => Reply {
            status: 302,
            headers: vec![("location", "/latest/".into())],
            body: String::new(),
        },
        _ => ok(page(&table("Latest", &good_row()))),
    }))
    .await;
    let rows = search(
        &runtime(&server.url, None),
        &server.url,
        &SearchQuery::default(),
    )
    .await
    .unwrap();
    assert_good_row(&rows[0], &server.url);
    assert_eq!(server.uris(), ["/home", "/latest/"]);

    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    let hostile = serve(Arc::new(move |_: &str| {
        counted.fetch_add(1, Ordering::Relaxed);
        ok(page(&table("Latest", &good_row())))
    }))
    .await;
    let target = format!("{}home", hostile.url);
    let server = serve(Arc::new(move |_: &str| Reply {
        status: 301,
        headers: vec![("location", target.clone())],
        body: String::new(),
    }))
    .await;
    let error = search(
        &runtime(&server.url, None),
        &server.url,
        &SearchQuery::default(),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("must remain on configured origin"),
        "{error}"
    );
    assert!(!error.contains(&hostile.url), "{error}");
    assert_eq!(hits.load(Ordering::Relaxed), 0);
    assert_eq!(server.uris(), ["/home"]);

    let server = serve(Arc::new(|_: &str| Reply {
        status: 302,
        headers: vec![("location", "/home".into())],
        body: String::new(),
    }))
    .await;
    let error = search(
        &runtime(&server.url, None),
        &server.url,
        &SearchQuery::default(),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("redirect limit exceeded"), "{error}");
    assert_eq!(server.uris(), vec!["/home"; 6]);
}

/// SYNTHETIC ONLY. There is no captured EZTV empty or error page, so none of these
/// bodies proves what a real zero-result page looks like. `table.forum_header_border`
/// is the row selector's table prefix (scraper cannot evaluate `:contains`), shown
/// only as a possible guard; production leaves `empty_html_container` at `None`.
/// This prefix guard still accepts an unrelated table without Latest/Releases;
/// it only rejects the examples that have no matching table at all.
#[tokio::test]
async fn zero_rows_and_missing_containers_are_characterized_synthetically() {
    let bodies = [
        ("empty latest table", page(&table("Latest", ""))),
        (
            "only an unrelated table",
            page(&table("Popular Shows", &good_row())),
        ),
        ("no table at all", page("<p>Maintenance</p>")),
        // Challenge-like interstitial without a cf-mitigated header.
        (
            "challenge-like body",
            page("<h1>Just a moment...</h1><form id=\"challenge-form\"></form>"),
        ),
    ];
    for (container, accepted) in [
        (None, [true, true, true, true]),
        (
            Some("table.forum_header_border"),
            [true, true, false, false],
        ),
    ] {
        for ((name, body), accepted) in bodies.iter().zip(accepted) {
            let body = body.clone();
            let server = serve(Arc::new(move |_: &str| ok(body.clone()))).await;
            let result = search(
                &runtime(&server.url, container),
                &server.url,
                &SearchQuery::default(),
            )
            .await;
            if accepted {
                assert!(result.unwrap().is_empty(), "{name} {container:?}");
            } else {
                let error = result.unwrap_err().to_string();
                assert!(error.contains("extraction failed"), "{name}: {error}");
            }
            assert_eq!(server.uris().len(), 1);
        }
    }
}

#[tokio::test]
async fn malformed_seeders_rows_are_skipped_including_all_bad_pages_and_noscript_parses() {
    let base = "https://eztvx.to/";
    let plan = plan(base);
    let ctx = plan.context(&BTreeMap::new(), &SearchQuery::default(), Utc::now());
    let request = plan.build_requests(&ctx).unwrap().remove(0);
    // Upstream skips only the failing row (CardigannIndexer.cs:1952-1955) and would
    // return the valid one. Offline extraction stays strict; the admitted runtime
    // opts into row skipping while preserving all response-wide failures.
    for seeders in ["N/A", "", "-"] {
        let bad = row(
            "/ep/9/bad/",
            "Bad Row [eztv]",
            &magnet(MAGNET_B),
            "1 GB",
            "1h",
            seeders,
        );
        let body = page(&table("Latest", &(good_row() + &bad)));
        let error = plan.extract(&request, &body, &ctx).unwrap_err();
        assert!(error.path.contains("seeders"), "{seeders:?}: {error:?}");
        let server = serve(Arc::new(move |_: &str| ok(body.clone()))).await;
        let rows = search(
            &runtime(&server.url, None),
            &server.url,
            &SearchQuery::default(),
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].title.starts_with("Show Name"));
        let body = page(&table("Latest", &bad));
        let server = serve(Arc::new(move |_: &str| ok(body.clone()))).await;
        let rows = search(
            &runtime(&server.url, None),
            &server.url,
            &SearchQuery::default(),
        )
        .await
        .unwrap();
        assert!(rows.is_empty());
    }
    // Noscript content is parsed as markup, as with Jackett's scripting-disabled
    // parser, so the `:contains` row selector still matches. The noscript content
    // contributes no release only because it holds no row the selector matches,
    // even a `Latest` table without magnet rows; nothing is stripped.
    let rows = table("Latest", &good_row());
    let analytics = "<noscript>Enable JavaScript<img src=\"/pixel.gif\" alt=\"Latest\">\
         <table class=\"forum_header_border\"><tr name=\"hover\" class=\"forum_header_border\">\
         <td>Latest</td></tr></table></noscript>";
    for body in [
        format!("<noscript>Enable JavaScript</noscript>{rows}"),
        format!("{analytics}{rows}"),
        format!("<noscript></noscript>{rows}"),
    ] {
        let releases = plan.extract(&request, &page(&body), &ctx).unwrap();
        assert_eq!(releases.len(), 1, "{body}");
        assert_eq!(releases[0].title, "Show Name S01E02 720p WEB H264-GROUP");
    }
    let noscript = page(&format!("{analytics}{rows}"));
    let server = serve(Arc::new(move |_: &str| ok(noscript.clone()))).await;
    let releases = search(
        &runtime(&server.url, None),
        &server.url,
        &SearchQuery::default(),
    )
    .await
    .unwrap();
    assert_eq!(releases.len(), 1);
}
