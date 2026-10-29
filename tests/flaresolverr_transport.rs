//! Loopback checks for the opt-in FlareSolverr fallback and saved-cookie composition.
//! The origin and the solver are local fakes; the solver replies use FlareSolverr 3.5.2's
//! `/v1` envelope shape. Pages are SYNTHETIC markup derived from the unchanged pinned
//! EZTV definition (as in `eztv_runtime.rs`). Nothing
//! here proves live tracker behavior or that a real challenge is solvable.
use rackett::{
    builtin::{self, Runtime},
    builtin_runtime::{flaresolverr::Solver, search_source, Source},
    cardigann::{self, parse, Limits},
    cardigann_exec::Plan,
    models::{Indexer, Release, SearchQuery},
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const SOURCE: &str = include_str!("../vendor/jackett/definitions/eztv.yml");
/// EZTV's pinned `search.headers.cookie`, verbatim.
const LAYOUT: &str = "sort_no=100; q_filter=all; q_filter_web=on; q_filter_reality=on; q_filter_x265=on; layout=def_wlinks";
/// Saved cookie: a private session value plus a conflicting `layout` the plan must win.
const SAVED: &str = "saved_secret=fixture-saved-value; layout=saved_layout";
const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) FixtureSolverBrowser/1.0";
const CLEARANCE: &str = "fixture-clearance-value";
const MAGNET: &str = "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567&dn=Show+Name";
const PRIVATE: &str = "fixture-private-body";

fn indexer(url: &str, cookie: &str) -> Indexer {
    Indexer {
        id: "eztv-fixture".into(),
        name: "EZTV fixture".into(),
        kind: "cardigann".into(),
        url: url.into(),
        cookie: cookie.into(),
        request_delay_ms: 0,
        definition: Some(json!({"builtin_id":"eztv","settings":{}})),
        ..Default::default()
    }
}
fn with_base(yaml: &str, base: &str) -> Plan {
    let mut root = parse(yaml, Limits::default()).unwrap();
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
    Plan::compile(&root).unwrap()
}
fn runtime(base: &str, delay_ms: u64) -> Runtime {
    Runtime {
        plan: Arc::new(with_base(SOURCE, base)),
        settings: BTreeMap::new(),
        delay_ms,
        modes: builtin::source_modes(&indexer(base, "")),
        empty_html_container: None,
        download: None,
    }
}
fn query(q: &str) -> SearchQuery {
    SearchQuery {
        params: BTreeMap::from([("q".into(), q.into())]),
    }
}
fn page() -> String {
    format!(
        "<!DOCTYPE html><html><head><title>EZTV</title></head><body>\
         <table class=\"forum_header_border\"><tr><td><h1>Releases</h1></td></tr>\
         <tr name=\"hover\" class=\"forum_header_border\">\
         <td><a href=\"/shows/1/\" title=\"Show Torrents\"></a></td>\
         <td><a href=\"/ep/1/show-name-s01e02/\" title=\"Show Name S01E02 720p [eztv] (2.5 GB)\">x</a></td>\
         <td><a href=\"{MAGNET}\" class=\"magnet\"></a></td>\
         <td>2.5 GB</td><td>3h 54m</td><td><font>57</font></td></tr></table></body></html>"
    )
}
const CHALLENGE: &str = "<!DOCTYPE html><html><head><title>Just a moment...</title></head><body><script>window._cf_chl_opt={};</script>fixture-private-body</body></html>";

#[derive(Clone, Debug)]
struct Hit {
    at: Instant,
    target: String,
    cookies: Vec<String>,
    agent: String,
}
struct Reply(u16, Vec<(&'static str, &'static str)>, String);
type Origin = Arc<dyn Fn(&Hit) -> Reply + Send + Sync>;
type SolverReply = Arc<dyn Fn(&Value, &str) -> (u16, String) + Send + Sync>;
struct Server {
    url: String,
    hits: Arc<Mutex<Vec<Hit>>>,
    solves: Arc<Mutex<Vec<(Instant, Value)>>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.tasks.iter().for_each(|t| t.abort());
    }
}
impl Server {
    fn hits(&self) -> Vec<Hit> {
        self.hits.lock().unwrap().clone()
    }
    fn solves(&self) -> Vec<Value> {
        self.solves
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.1.clone())
            .collect()
    }
}
async fn bind() -> (tokio::net::TcpListener, String) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", l.local_addr().unwrap());
    (l, url)
}
/// An origin and a solver; the solver reply gets the request JSON and the origin URL.
async fn serve(origin: Origin, solver: SolverReply, solver_delay: Duration) -> (Server, Solver) {
    let (ol, url) = bind().await;
    let (sl, solver_url) = bind().await;
    let hits = Arc::new(Mutex::new(Vec::new()));
    let solves = Arc::new(Mutex::new(Vec::new()));
    let captured = hits.clone();
    let origin_app = axum::Router::new().fallback(
        move |uri: axum::http::Uri, headers: axum::http::HeaderMap| {
            let (origin, captured) = (origin.clone(), captured.clone());
            async move {
                let hit = Hit {
                    at: Instant::now(),
                    target: uri.path_and_query().unwrap().to_string(),
                    cookies: headers
                        .get_all("cookie")
                        .iter()
                        .map(|v| v.to_str().unwrap().to_owned())
                        .collect(),
                    agent: headers
                        .get("user-agent")
                        .map(|v| v.to_str().unwrap().to_owned())
                        .unwrap_or_default(),
                };
                captured.lock().unwrap().push(hit.clone());
                let Reply(status, headers, body) = origin(&hit);
                let mut response = axum::response::IntoResponse::into_response(body);
                *response.status_mut() = axum::http::StatusCode::from_u16(status).unwrap();
                response.headers_mut().clear();
                response
                    .headers_mut()
                    .insert("content-type", "text/html; charset=utf-8".parse().unwrap());
                for (k, v) in headers {
                    response.headers_mut().insert(k, v.parse().unwrap());
                }
                response
            }
        },
    );
    let (base, captured) = (url.clone(), solves.clone());
    let solver_app = axum::Router::new().route(
        "/v1",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let (solver, captured, base) = (solver.clone(), captured.clone(), base.clone());
            async move {
                captured
                    .lock()
                    .unwrap()
                    .push((Instant::now(), body.clone()));
                tokio::time::sleep(solver_delay).await;
                let (status, text) = solver(&body, &base);
                let mut response = axum::response::IntoResponse::into_response(text);
                *response.status_mut() = axum::http::StatusCode::from_u16(status).unwrap();
                response
            }
        }),
    );
    let tasks = vec![
        tokio::spawn(async move { axum::serve(ol, origin_app).await.unwrap() }),
        tokio::spawn(async move { axum::serve(sl, solver_app).await.unwrap() }),
    ];
    let server = Server {
        url,
        hits,
        solves,
        tasks,
    };
    (server, Solver::new(&solver_url).unwrap())
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
/// A FlareSolverr 3.5.2-shaped success for the requested URL.
fn solved(body: &Value, page: &str, cookies: Value) -> (u16, String) {
    let reply = json!({
        "status": "ok",
        "message": "Challenge solved!",
        "solution": {
            "url": body["url"],
            "status": 200,
            "headers": {},
            "response": page,
            "cookies": cookies,
            "userAgent": UA,
        },
        "startTimestamp": 1, "endTimestamp": 2, "version": "3.5.2"
    });
    (200, reply.to_string())
}
fn clearance_cookie() -> Value {
    json!([{"name":"cf_clearance","value":CLEARANCE,"domain":"127.0.0.1","path":"/","secure":false,"httpOnly":true,"expiry":now()+3600,"sameSite":"None"}])
}
/// Origin: challenge unless the request carries the solver's clearance.
fn guarded(status: u16) -> Origin {
    Arc::new(move |hit: &Hit| {
        if hit.cookies.iter().any(|c| c.contains(CLEARANCE)) {
            Reply(200, vec![], page())
        } else if status == 403 {
            Reply(403, vec![("cf-mitigated", "challenge")], CHALLENGE.into())
        } else {
            // Marker-only interstitial: no cf-mitigated header.
            Reply(status, vec![], CHALLENGE.into())
        }
    })
}
async fn run(
    rt: &Runtime,
    url: &str,
    source: &Source,
    q: &str,
    deadline: Duration,
) -> anyhow::Result<Vec<Release>> {
    // The engine's bundled-source client policy, including its default user agent.
    let client = reqwest::Client::builder()
        .user_agent("Rackett/0.1")
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .unwrap();
    search_source(
        rt,
        &client,
        &indexer(url, SAVED),
        &query(q),
        &mut Instant::now(),
        source,
        Instant::now() + deadline,
    )
    .await
}
fn assert_hidden(error: &anyhow::Error) {
    let text = format!("{error} {error:?} {error:#}");
    for secret in [
        "fixture-saved-value",
        CLEARANCE,
        "FixtureSolverBrowser",
        PRIVATE,
        "Show-Name",
        "127.0.0.1",
    ] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
}

#[test]
fn solver_endpoint_is_explicit_and_validated() {
    for ok in [
        "http://flaresolverr:8191",
        "http://flaresolverr:8191/",
        "http://127.0.0.1:8191/v1",
        "https://solver.example/",
    ] {
        assert!(Solver::new(ok).is_ok(), "{ok}");
    }
    for bad in [
        "",
        "flaresolverr:8191",
        "ftp://flaresolverr/",
        "http://user:secret-pass@flaresolverr:8191/",
        "http://flaresolverr:8191/?token=secret-pass",
        "http://flaresolverr:8191/#secret-pass",
        "http://flaresolverr:8191/other",
        "http://flaresolverr:8191/v1/",
    ] {
        let Err(error) = Solver::new(bad) else {
            panic!("{bad}")
        };
        assert!(!error.to_string().contains("secret-pass"), "{error}");
        assert!(error.to_string().contains("RACKETT_FLARESOLVERR_URL"));
    }
    // No implicit endpoint: unset means disabled (skipped if this shell configures one).
    if std::env::var_os("RACKETT_FLARESOLVERR_URL").is_none() {
        assert!(Solver::configured().unwrap().is_none());
    }
}

#[test]
fn saved_cookie_is_admitted_only_for_challenge_ids() {
    assert_eq!(builtin::CHALLENGE_IDS, ["1337x", "eztv", "tokyotosho"]);
    let i = indexer("https://eztvx.to/", SAVED);
    assert!(Runtime::compile(&i).is_ok());
    let mut x = i.clone();
    x.definition = Some(json!({"builtin_id":"1337x","settings":{}}));
    assert!(Runtime::compile(&x).is_ok());
    assert!(builtin::READY_IDS.contains(&"eztv") && builtin::READY_IDS.contains(&"1337x"));
    // Malformed cookie text fails with a static message.
    for bad in [
        "novalue-secretv",
        "a=secretv; a=2",
        "a=\"secretv\"",
        "a=secretv;",
        "a=secretv; b=__jackett_expired__",
        "a=secret\tv",
    ] {
        for id in ["eztv", "1337x", "tokyotosho"] {
            x.cookie = bad.into();
            x.definition = Some(json!({"builtin_id":id,"settings":{}}));
            let error = Runtime::compile(&x).err().unwrap().to_string();
            assert_eq!(error, "Invalid cookie setting", "{bad}");
        }
    }
    // Other bundled IDs and API keys keep the generic-credential rejection.
    for id in ["acgrip", "uindex", "milkie"] {
        let mut other = indexer("https://source.example/", "a=1");
        other.definition = Some(json!({"builtin_id":id,"settings":{}}));
        let error = Runtime::compile(&other).err().unwrap().to_string();
        assert!(error.contains("generic credentials"), "{id}: {error}");
        assert!(Source::new(&other, None).is_err());
    }
    let mut keyed = indexer("https://eztvx.to/", "");
    keyed.api_key = "secret-key".into();
    let error = Runtime::compile(&keyed).err().unwrap().to_string();
    assert!(error.contains("generic credentials"), "{error}");
}

#[tokio::test]
async fn disabled_by_default_composes_one_cookie_and_keeps_challenge_errors() {
    let (server, _) = serve(
        Arc::new(|hit: &Hit| {
            if hit.target.starts_with("/search/Challenged") {
                Reply(403, vec![("cf-mitigated", "challenge")], CHALLENGE.into())
            } else {
                Reply(200, vec![], page())
            }
        }),
        Arc::new(|_, _| unreachable!()),
        Duration::ZERO,
    )
    .await;
    let rt = runtime(&server.url, 0);
    let source = Source::new(&indexer(&server.url, SAVED), None).unwrap();
    let rows = run(
        &rt,
        &server.url,
        &source,
        "Show Name",
        Duration::from_secs(20),
    )
    .await
    .unwrap();
    assert_eq!(rows[0].title, "Show Name S01E02 720p");
    // One header: every pinned layout pair, then saved pairs whose names are new.
    assert_eq!(
        server.hits()[0].cookies,
        [format!("{LAYOUT}; saved_secret=fixture-saved-value")]
    );
    assert_eq!(server.hits()[0].agent, "Rackett/0.1");
    // Without a saved cookie the plan's own header is sent exactly as before.
    let plain = Source::new(&indexer(&server.url, ""), None).unwrap();
    run(
        &rt,
        &server.url,
        &plain,
        "Show Name",
        Duration::from_secs(20),
    )
    .await
    .unwrap();
    assert_eq!(server.hits()[1].cookies, [LAYOUT]);
    // No solver: the challenge is the existing static error, after one request.
    let error = run(
        &rt,
        &server.url,
        &source,
        "Challenged",
        Duration::from_secs(20),
    )
    .await
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("Cloudflare challenge detected; challenge solving is not supported"));
    assert_hidden(&error);
    assert_eq!(server.hits().len(), 3);
}

#[tokio::test]
async fn known_challenge_gets_one_solver_post_then_clearance_follow_up() {
    for status in [200, 403, 503] {
        let (server, solver) = serve(
            guarded(status),
            Arc::new(|body, _| solved(body, &page(), clearance_cookie())),
            Duration::ZERO,
        )
        .await;
        let rt = runtime(&server.url, 150);
        let source = Source::new(&indexer(&server.url, SAVED), Some(Arc::new(solver))).unwrap();
        let rows = run(
            &rt,
            &server.url,
            &source,
            "Show Name",
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        // The rendered page is extracted directly; no extra origin fetch.
        assert_eq!(rows.len(), 1, "{status}");
        assert_eq!(rows[0].title, "Show Name S01E02 720p");
        assert_eq!(rows[0].link, MAGNET);
        assert_eq!(
            rows[0].details.as_deref(),
            Some(&*format!("{}ep/1/show-name-s01e02/", server.url))
        );
        assert_eq!(server.hits().len(), 1);
        let solves = server.solves();
        assert_eq!(solves.len(), 1);
        let request = &solves[0];
        assert_eq!(request["cmd"], "request.get");
        assert_eq!(request["url"], format!("{}search/Show-Name", server.url));
        let max = request["maxTimeout"].as_u64().unwrap();
        assert!((1000..=19000).contains(&max), "{max}");
        // Only the plan's static layout pairs reach the solver; never the saved cookie.
        let names: Vec<_> = request["cookies"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                format!(
                    "{}={}",
                    c["name"].as_str().unwrap(),
                    c["value"].as_str().unwrap()
                )
            })
            .collect();
        assert_eq!(names.join("; "), LAYOUT);
        assert!(!request.to_string().contains("fixture-saved-value"));
        assert_eq!(
            request.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["cmd", "cookies", "maxTimeout", "url"]
        );
        // The solver call takes its own gate turn after the origin request.
        let solved_at = server.solves.lock().unwrap()[0].0;
        assert!(solved_at.duration_since(server.hits()[0].at) >= Duration::from_millis(140));
        // Follow-up: cached clearance and its user agent, one Cookie header, no solver.
        run(
            &rt,
            &server.url,
            &source,
            "Show Name",
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        let hits = server.hits();
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[1].cookies,
            [format!(
                "{LAYOUT}; cf_clearance={CLEARANCE}; saved_secret=fixture-saved-value"
            )]
        );
        assert_eq!(hits[1].agent, UA);
        assert_eq!(server.solves().len(), 1);
    }
}

#[tokio::test]
async fn ordinary_pages_unrelated_errors_post_and_json_never_call_the_solver() {
    let origin: Origin = Arc::new(|hit: &Hit| match hit.target.as_str() {
        t if t.starts_with("/search/Forbidden") => Reply(403, vec![], format!("denied {PRIVATE}")),
        t if t.starts_with("/search/Down") => Reply(503, vec![], "maintenance".into()),
        // Markers alone, but a status that is never treated as a challenge.
        t if t.starts_with("/search/Missing") => Reply(404, vec![], CHALLENGE.into()),
        // An ordinary Cloudflare-served page with the injected script probe.
        _ => Reply(
            200,
            vec![("server", "cloudflare")],
            page().replace(
                "</body>",
                "<script src=\"/cdn-cgi/challenge-platform/scripts/jsd/main.js\"></script></body>",
            ),
        ),
    });
    let (server, solver) = serve(origin, Arc::new(|_, _| unreachable!()), Duration::ZERO).await;
    let rt = runtime(&server.url, 0);
    let source = Source::new(&indexer(&server.url, SAVED), Some(Arc::new(solver))).unwrap();
    assert_eq!(
        run(
            &rt,
            &server.url,
            &source,
            "Show Name",
            Duration::from_secs(20)
        )
        .await
        .unwrap()
        .len(),
        1
    );
    for (q, expected) in [
        ("Forbidden", "HTTP 403 Forbidden: Access denied"),
        ("Down", "HTTP 503 Service Unavailable"),
        ("Missing", "HTTP 404 Not Found"),
    ] {
        let error = run(&rt, &server.url, &source, q, Duration::from_secs(20))
            .await
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
        assert_hidden(&error);
    }
    assert_eq!(server.hits().len(), 4);
    // POST and JSON plans: the challenge stays the existing error, nothing is replayed.
    for path in [
        "{path: search, method: post}",
        "{path: search, response: {type: json}}",
    ] {
        let (challenged, solver) = serve(
            guarded(403),
            Arc::new(|_, _| unreachable!()),
            Duration::ZERO,
        )
        .await;
        let yaml = format!("id: eztv\nname: Fixture\ndescription: Synthetic\nlanguage: en-US\ntype: public\nencoding: UTF-8\nlinks: [{}]\ncaps:\n  categories: {{1: TV}}\n  modes: {{search: [q]}}\nsettings: []\nsearch:\n  paths: [{path}]\n  rows: {{selector: tr}}\n  fields:\n    category: {{text: 1}}\n    size: {{text: 1}}\n    seeders: {{text: 1}}\n    title: {{text: x}}\n    download: {{text: x}}\n", challenged.url);
        let rt = Runtime {
            plan: Arc::new(with_base(&yaml, &challenged.url)),
            settings: BTreeMap::new(),
            delay_ms: 0,
            modes: BTreeMap::from([("search".into(), vec!["q".into()])]),
            empty_html_container: None,
            download: None,
        };
        let source = Source::new(&indexer(&challenged.url, ""), Some(Arc::new(solver))).unwrap();
        let error = run(&rt, &challenged.url, &source, "x", Duration::from_secs(20))
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("challenge solving is not supported"),
            "{error}"
        );
        assert_eq!(challenged.hits().len(), 1);
        assert!(challenged.solves().is_empty());
    }
    assert!(server.solves().is_empty());
}

#[tokio::test]
async fn invalid_solver_results_fail_closed_without_leaks_or_retries() {
    let big = "x".repeat(16 * 1024 * 1024 + 1);
    let cases: Vec<(SolverReply, &str)> = vec![
        (
            Arc::new(|_, _| {
                (200, json!({"status":"ok","solution":{"url":"http://other.example/search/Show-Name","status":200,"response":page(),"cookies":[],"userAgent":UA}}).to_string())
            }),
            "another origin",
        ),
        (
            Arc::new(move |b, _| solved(b, &"<html>".repeat(8 * 1024 * 1024 / 6 + 1), json!([]))),
            "exceeds 8 MiB",
        ),
        (
            Arc::new(move |_, _| (200, big.clone())),
            "exceeds its size limit",
        ),
        (
            Arc::new(|_, _| (200, "{\"status\":\"ok\"".into())),
            "malformed",
        ),
        (
            Arc::new(|_, _| (200, json!({"status":"ok"}).to_string())),
            "malformed",
        ),
        (
            Arc::new(|_, _| {
                (
                    500,
                    json!({"status":"error","message":format!("Error solving {PRIVATE}")})
                        .to_string(),
                )
            }),
            "could not solve",
        ),
        (
            Arc::new(|_, _| (200, json!({"status":"error","message":"x"}).to_string())),
            "could not solve",
        ),
        (
            Arc::new(|b, _| {
                let (s, text) = solved(b, &page(), clearance_cookie());
                (s, text.replace("\"status\":200", "\"status\":403"))
            }),
            "HTTP error",
        ),
        (
            Arc::new(|b, _| solved(b, CHALLENGE, clearance_cookie())),
            "not solved",
        ),
        (Arc::new(|b, _| solved(b, "", json!([]))), "malformed"),
    ];
    for (reply, expected) in cases {
        let (server, solver) = serve(guarded(403), reply, Duration::ZERO).await;
        let rt = runtime(&server.url, 0);
        let source = Source::new(&indexer(&server.url, SAVED), Some(Arc::new(solver))).unwrap();
        let error = run(
            &rt,
            &server.url,
            &source,
            "Show Name",
            Duration::from_secs(20),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains(expected), "{expected}: {error}");
        assert_hidden(&error);
        assert_eq!(
            (server.hits().len(), server.solves().len()),
            (1, 1),
            "{expected}"
        );
        // A failed solve caches nothing: the next request is challenged and solved again.
        let _ = run(
            &rt,
            &server.url,
            &source,
            "Show Name",
            Duration::from_secs(20),
        )
        .await;
        assert!(server.hits()[1]
            .cookies
            .iter()
            .all(|c| !c.contains(CLEARANCE)));
    }
    // Unreachable solver endpoint.
    let (server, _) = serve(
        guarded(403),
        Arc::new(|_, _| unreachable!()),
        Duration::ZERO,
    )
    .await;
    let (closed, url) = bind().await;
    drop(closed);
    let source = Source::new(
        &indexer(&server.url, SAVED),
        Some(Arc::new(Solver::new(&url).unwrap())),
    )
    .unwrap();
    let error = run(
        &runtime(&server.url, 0),
        &server.url,
        &source,
        "Show Name",
        Duration::from_secs(20),
    )
    .await
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Challenge solver transport failed"),
        "{error}"
    );
    assert_hidden(&error);
}

#[tokio::test]
async fn clearance_is_narrowed_and_isolated_per_source_epoch() {
    // Only the exact-host, path-/, unexpired, non-secure-over-HTTP pair is kept.
    let cookies = json!([
        {"name":"cf_clearance","value":CLEARANCE,"domain":".127.0.0.1","path":"/","secure":false},
        {"name":"foreign","value":"x1","domain":"other.example","path":"/"},
        {"name":"pathed","value":"x2","domain":"127.0.0.1","path":"/admin"},
        {"name":"securehttp","value":"x3","domain":"127.0.0.1","path":"/","secure":true},
        {"name":"expired","value":"x4","domain":"127.0.0.1","path":"/","expiry":1},
        {"name":"overflow","value":"x-overflow","domain":"127.0.0.1","path":"/","expiry":1e300},
        {"name":"bad value","value":"x5","domain":"127.0.0.1","path":"/"},
        {"name":"nodomain","value":"x6","path":"/"},
        {"name":"layout","value":"solver_layout","domain":"127.0.0.1","path":"/"}
    ]);
    let (server, solver) = serve(
        guarded(403),
        Arc::new(move |b, _| solved(b, &page(), cookies.clone())),
        Duration::ZERO,
    )
    .await;
    let solver = Arc::new(solver);
    let rt = runtime(&server.url, 0);
    let i = indexer(&server.url, SAVED);
    let a = Source::new(&i, Some(solver.clone())).unwrap();
    run(&rt, &server.url, &a, "Show Name", Duration::from_secs(20))
        .await
        .unwrap();
    run(&rt, &server.url, &a, "Show Name", Duration::from_secs(20))
        .await
        .unwrap();
    assert_eq!(
        server.hits()[1].cookies,
        [format!(
            "{LAYOUT}; cf_clearance={CLEARANCE}; saved_secret=fixture-saved-value"
        )]
    );
    assert_eq!(server.solves().len(), 1);
    // Another source, or this source's replacement runtime after an edit or restart,
    // starts without clearance and is challenged again.
    for other in [
        Source::new(&i, Some(solver.clone())).unwrap(),
        Source::new(&i, Some(solver.clone())).unwrap(),
    ] {
        let before = server.hits().len();
        run(
            &rt,
            &server.url,
            &other,
            "Show Name",
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        assert!(server.hits()[before]
            .cookies
            .iter()
            .all(|c| !c.contains(CLEARANCE)));
    }
    assert_eq!(server.solves().len(), 3);
    // Solver cookies that are all expired leave nothing cached.
    let (server, solver) = serve(
        guarded(403),
        Arc::new(|b, _| solved(b, &page(), json!([{"name":"cf_clearance","value":CLEARANCE,"domain":"127.0.0.1","path":"/","expiry":1}]))),
        Duration::ZERO,
    )
    .await;
    let source = Source::new(&indexer(&server.url, ""), Some(Arc::new(solver))).unwrap();
    for _ in 0..2 {
        run(
            &rt_for(&server.url),
            &server.url,
            &source,
            "Show Name",
            Duration::from_secs(20),
        )
        .await
        .unwrap();
    }
    assert_eq!(server.solves().len(), 2);
    assert!(server.hits().iter().all(|h| h.agent == "Rackett/0.1"));
}
fn rt_for(url: &str) -> Runtime {
    runtime(url, 0)
}

#[tokio::test]
async fn deadline_bounds_the_solve_and_each_request_solves_at_most_once() {
    // Too little time left: no solver request at all.
    let (server, solver) = serve(
        guarded(403),
        Arc::new(|_, _| unreachable!()),
        Duration::ZERO,
    )
    .await;
    let source = Source::new(&indexer(&server.url, SAVED), Some(Arc::new(solver))).unwrap();
    let error = run(
        &runtime(&server.url, 0),
        &server.url,
        &source,
        "Show Name",
        Duration::from_millis(1500),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("no time to solve"), "{error}");
    assert!(server.solves().is_empty());
    // A slow solver is cut off at the caller's deadline; maxTimeout fits inside it.
    let (server, solver) = serve(
        guarded(403),
        Arc::new(|b, _| solved(b, &page(), clearance_cookie())),
        Duration::from_secs(30),
    )
    .await;
    let source = Source::new(&indexer(&server.url, SAVED), Some(Arc::new(solver))).unwrap();
    let start = Instant::now();
    let error = run(
        &runtime(&server.url, 0),
        &server.url,
        &source,
        "Show Name",
        Duration::from_secs(3),
    )
    .await
    .unwrap_err();
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(
        error
            .to_string()
            .contains("Challenge solver transport failed"),
        "{error}"
    );
    assert_hidden(&error);
    let max = server.solves()[0]["maxTimeout"].as_u64().unwrap();
    assert!((1000..=2000).contains(&max), "{max}");
    // The solver's own maximum caps long deadlines.
    let (server, solver) = serve(
        guarded(403),
        Arc::new(|b, _| solved(b, &page(), clearance_cookie())),
        Duration::ZERO,
    )
    .await;
    let source = Source::new(&indexer(&server.url, SAVED), Some(Arc::new(solver))).unwrap();
    run(
        &runtime(&server.url, 0),
        &server.url,
        &source,
        "Show Name",
        Duration::from_secs(120),
    )
    .await
    .unwrap();
    assert_eq!(server.solves()[0]["maxTimeout"], 60000);
    // An origin that keeps challenging despite clearance: one solve per request, never more.
    let (server, solver) = serve(
        Arc::new(|_: &Hit| Reply(403, vec![("cf-mitigated", "challenge")], CHALLENGE.into())),
        Arc::new(|b, _| solved(b, &page(), clearance_cookie())),
        Duration::ZERO,
    )
    .await;
    let source = Source::new(&indexer(&server.url, SAVED), Some(Arc::new(solver))).unwrap();
    for n in 1..=2 {
        run(
            &runtime(&server.url, 0),
            &server.url,
            &source,
            "Show Name",
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        assert_eq!((server.hits().len(), server.solves().len()), (n, n));
    }
}
