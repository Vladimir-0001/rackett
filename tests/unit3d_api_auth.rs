use rackett::{
    builtin,
    builtin_runtime::api_auth::{self, ApiSession},
    cardigann::{self, Node},
    cardigann_exec::request::BuiltRequest,
};
fn raw() -> &'static str {
    builtin::SOURCES
        .iter()
        .find(|(id, _)| *id == "0dayfiles-api")
        .unwrap()
        .1
}
fn parse(s: &str) -> Node {
    cardigann::parse(s, cardigann::Limits::default()).unwrap()
}
#[test]
fn entire_api_login_shape_and_key_are_checked_before_network() {
    api_auth::validate_definition(&parse(raw())).unwrap();
    for (from, to) in [
        ("method: get", "method: post"),
        ("path: api/torrents\n", "path: api/other\n"),
        (
            "path: api/torrents\n",
            "path: api/torrents\n  inputs: {x: y}\n",
        ),
        ("a[href*=\"/login\"]", "a[href*=\"/logout\"]"),
        ("Account is Banned", "Other text"),
        (
            "The API key was not accepted by {{ .Config.sitelink }}.",
            "Other message",
        ),
        (
            "Authorization: [\"Bearer {{ .Config.apikey }}\"]",
            "Authorization: [\"Bearer {{ .Config.apikey }}\", \"other\"]",
        ),
        (
            "Authorization: [\"Bearer {{ .Config.apikey }}\"]",
            "Authorization: [\"Bearer {{ .Config.apikey }}\"]\n    Cookie: [\"x=y\"]",
        ),
        (
            "path: api/torrents/filter",
            "path: https://other.invalid/api",
        ),
        ("type: json", "type: xml"),
        ("attribute: attributes", "attribute: other"),
        ("selector: data\n", "selector: other\n"),
        ("encoding: UTF-8", "encoding: iso-8859-1"),
        (
            "id: 0dayfiles-api",
            "id: 0dayfiles-api\nfollowredirect: true",
        ),
        ("id: 0dayfiles-api", "id: 0dayfiles-api\ndownload: {}"),
        ("method: get", "method: get\n  method: get"),
        ("apikey\n    type: text", "apikey\n    type: checkbox"),
    ] {
        assert!(raw().contains(from), "mutation absent: {from}");
        assert!(
            api_auth::validate_definition(&parse(&raw().replace(from, to))).is_err(),
            "accepted {from} -> {to}"
        );
    }
    let root = parse(raw());
    for key in ["", " ", "x y", "x\ny", "x\r\ny", "é", &"x".repeat(4097)] {
        assert!(ApiSession::new(&root, "http://127.0.0.1:1/", key).is_err());
    }
    for base in [
        "ftp://example.org/",
        "https://user:secret@example.org/",
        "https://example.org/?secret=x",
        "https://example.org/#x",
    ] {
        assert!(ApiSession::new(&root, base, "fixture-key").is_err());
    }
}
#[tokio::test]
async fn api_transport_bounds_redirects_scoped_cookies_delay_and_rejects_bad_requests() {
    use axum::{
        extract::State,
        http::{HeaderMap, StatusCode, Uri},
        response::{IntoResponse, Response},
        routing::get,
        Router,
    };
    use std::{
        sync::{
            atomic::{AtomicU8, AtomicUsize, Ordering},
            Arc, Mutex,
        },
        time::{Duration, Instant},
    };
    struct Fixture {
        mode: AtomicU8,
        hits: Mutex<Vec<(String, Instant)>>,
        foreign: String,
    }
    async fn handler(State(f): State<Arc<Fixture>>, uri: Uri, h: HeaderMap) -> Response {
        assert_eq!(h.get("authorization").unwrap(), "Bearer fixture-key");
        f.hits
            .lock()
            .unwrap()
            .push((uri.to_string(), Instant::now()));
        let mode = f.mode.load(Ordering::SeqCst);
        if uri.path() == "/api/torrents" {
            assert!(!h.contains_key("cookie"));
            return match mode {
                1 => ([("refresh", "0;url=/login")], "{\"data\":[]}").into_response(),
                2 => ([("cf-mitigated", "challenge")], "{\"data\":[]}").into_response(),
                3 => (
                    [("content-type", "application/json; charset=windows-1251")],
                    "{\"data\":[]}",
                )
                    .into_response(),
                4 => vec![0xff].into_response(),
                5 => "{\"data\":[],\"notice\":\"Account is Banned secret\"}".into_response(),
                6 => "x".repeat(8 * 1024 * 1024 + 1).into_response(),
                _ => ([("set-cookie", "session=fixture; Path=/")], "{\"data\":[]}").into_response(),
            };
        }
        assert_eq!(h.get("cookie").unwrap(), "session=fixture");
        match uri.path() {
            "/api/torrents/filter" => axum::Json(serde_json::json!({"data":[]})).into_response(),
            "/torrent/redirect" => {
                (StatusCode::FOUND, [("location", "/torrent/file")], "").into_response()
            }
            "/torrent/foreign" => {
                (StatusCode::FOUND, [("location", f.foreign.as_str())], "").into_response()
            }
            "/torrent/loop" => {
                (StatusCode::FOUND, [("location", "/torrent/loop")], "").into_response()
            }
            "/torrent/file" => b"d4:infod4:name7:fixture6:lengthi42eee"
                .as_slice()
                .into_response(),
            _ => StatusCode::NOT_FOUND.into_response(),
        }
    }
    let foreign_hits = Arc::new(AtomicUsize::new(0));
    let hits = foreign_hits.clone();
    let foreign = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let foreign_url = format!("http://{}/", foreign.local_addr().unwrap());
    let app = Router::new().fallback(get(move || {
        let hits = hits.clone();
        async move {
            hits.fetch_add(1, Ordering::SeqCst);
            "unexpected"
        }
    }));
    let foreign_server = tokio::spawn(async move { axum::serve(foreign, app).await.unwrap() });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let fixture = Arc::new(Fixture {
        mode: AtomicU8::new(0),
        hits: Default::default(),
        foreign: foreign_url.clone(),
    });
    let app = Router::new()
        .fallback(get(handler))
        .with_state(fixture.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let session = ApiSession::new(&parse(raw()), &base, "fixture-key").unwrap();
    let mut request = BuiltRequest::html_get(format!("{base}api/torrents/filter"));
    request.response_type = "json".into();
    request.headers = vec![("Authorization".into(), "Bearer fixture-key".into())];
    let mut gate = Instant::now();
    for mutant in 0..5 {
        let mut bad = request.clone();
        match mutant {
            0 => bad.url = foreign_url.clone(),
            1 => bad.method = "POST".into(),
            2 => bad.headers.clear(),
            3 => bad.headers[0].1 = "Bearer other".into(),
            _ => bad.url = format!("{base}other"),
        };
        assert!(session
            .fetch_search(&client, &bad, &mut gate, 0)
            .await
            .is_err());
        assert!(fixture.hits.lock().unwrap().is_empty());
    }
    session
        .fetch_search(&client, &request, &mut gate, 30)
        .await
        .unwrap();
    let hits = std::mem::take(&mut *fixture.hits.lock().unwrap());
    assert_eq!(hits.len(), 2);
    assert!(hits[1].1.duration_since(hits[0].1) >= Duration::from_millis(30));
    for mode in 1..=6 {
        fixture.mode.store(mode, Ordering::SeqCst);
        let e = session
            .fetch_search(&client, &request, &mut gate, 0)
            .await
            .err()
            .expect("auth failure expected");
        assert!(!e.to_string().contains("secret"));
        assert!(!e.to_string().contains("fixture-key"));
        assert_eq!(std::mem::take(&mut *fixture.hits.lock().unwrap()).len(), 1);
    }
    fixture.mode.store(0, Ordering::SeqCst);
    let bytes = session
        .fetch_download(&client, &format!("{base}torrent/redirect"), &mut gate, 0)
        .await
        .unwrap();
    rackett::torrent::validate(&bytes).unwrap();
    assert_eq!(std::mem::take(&mut *fixture.hits.lock().unwrap()).len(), 3);
    assert!(session
        .fetch_download(&client, &format!("{base}torrent/foreign"), &mut gate, 0)
        .await
        .is_err());
    assert_eq!(std::mem::take(&mut *fixture.hits.lock().unwrap()).len(), 2);
    assert_eq!(foreign_hits.load(Ordering::SeqCst), 0);
    assert!(session
        .fetch_download(&client, &format!("{base}torrent/loop"), &mut gate, 0)
        .await
        .is_err());
    assert_eq!(std::mem::take(&mut *fixture.hits.lock().unwrap()).len(), 7);
    server.abort();
    foreign_server.abort();
}
