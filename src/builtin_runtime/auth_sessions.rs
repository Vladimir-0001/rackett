//! Storage and cancellation tests use synthetic, isolated sessions only.
use super::*;
use axum::{extract::State, http::HeaderMap, response::IntoResponse, routing::get, Router};
use reqwest::header::SET_COOKIE;

fn bound(
    repository: &SessionRepository,
    source: &str,
    builtin: &str,
    base: &str,
    seed: &str,
) -> CookieSession {
    let mut session = CookieSession::new(
        CookiePlan {
            aidoru_fragment: false,
            user_agent: None,
            declared: Vec::new(),
            header_settings: Default::default(),
            headers: Vec::new(),
            charset: crate::cardigann_exec::request::Charset::Utf8,
            certificates: None,
            test: Some(LoginTest {
                path: "index.php".into(),
                selector: Some("a[href='logout.php']".into()),
            }),
        },
        base,
        seed,
        0,
        // These tests assert storage and cookie behavior, not elapsed deadlines.
        // Concurrent CI builds can delay blocking fsync work beyond five seconds.
        120,
    )
    .unwrap();
    session.bind(repository, source, builtin).unwrap();
    session
}
fn change(state: &mut SessionState, url: &url::Url, value: &str) {
    let mut headers = HeaderMap::new();
    headers.insert(SET_COOKIE, value.parse().unwrap());
    state.dirty |= state
        .cookies
        .update(url, &headers, chrono::Utc::now().timestamp())
        .unwrap();
}
fn header(state: &mut SessionState, url: &url::Url) -> String {
    state
        .cookies
        .header(url, chrono::Utc::now().timestamp())
        .unwrap()
        .map(|h| h.to_str().unwrap().to_owned())
        .unwrap_or_default()
}

#[tokio::test]
async fn form_credentials_scope_durable_sessions_and_survive_restart() {
    use super::super::form_auth::FormPlan;
    let source = include_str!("../../vendor/jackett/definitions/4thd.yml");
    let make = |repository: &SessionRepository, password: &str| {
        let node = crate::cardigann::parse(source, crate::cardigann::Limits::default()).unwrap();
        let settings = std::collections::BTreeMap::from([
            ("username".into(), serde_json::json!("fixture-user")),
            ("password".into(), serde_json::json!(password)),
        ]);
        let mut session = CookieSession::new_form(
            FormPlan::compile(&node).unwrap(),
            "https://tracker.example/",
            &settings,
            0,
            3,
        )
        .unwrap();
        session.bind(repository, "one", "4thd").unwrap();
        session
    };
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let repository = SessionRepository::new(&config);
    let first = make(&repository, "old-password");
    let same = make(&repository, "old-password");
    assert!(Arc::ptr_eq(&first.state, &same.state));
    let mut state = first
        .durable(first.state.clone().lock_owned().await)
        .await
        .unwrap();
    assert!(header(&mut state, &first.base).is_empty());
    change(&mut state, &first.base, "session=authenticated; Path=/");
    drop(first.durable(state).await.unwrap());
    let changed = make(&repository, "new-password");
    assert!(!Arc::ptr_eq(&first.state, &changed.state));
    let mut state = changed
        .durable(changed.state.clone().lock_owned().await)
        .await
        .unwrap();
    assert!(header(&mut state, &changed.base).is_empty());
    drop(state);
    drop(first);
    drop(same);
    drop(changed);
    drop(repository);
    let restarted = make(&SessionRepository::new(&config), "old-password");
    let mut state = restarted
        .durable(restarted.state.clone().lock_owned().await)
        .await
        .unwrap();
    assert_eq!(header(&mut state, &restarted.base), "session=authenticated");
    // Neither filenames nor the session payload retain the supplied password.
    for entry in std::fs::read_dir(config.with_extension("json.sessions")).unwrap() {
        let entry = entry.unwrap();
        assert!(!entry.file_name().to_string_lossy().contains("password"));
        let text = std::fs::read_to_string(entry.path()).unwrap();
        assert!(!text.contains("old-password") && !text.contains("new-password"));
    }
}

#[tokio::test]
async fn identities_share_only_the_same_source_origin_definition_and_seed() {
    let dir = tempfile::tempdir().unwrap();
    let repository = SessionRepository::new(&dir.path().join("config.json"));
    let base = "https://tracker.example/";
    let first = bound(&repository, "one", "musopia", base, "session=seed");
    let second = bound(&repository, "one", "musopia", base, "session=seed");
    assert!(Arc::ptr_eq(&first.state, &second.state));
    let mut state = first
        .durable(first.state.clone().lock_owned().await)
        .await
        .unwrap();
    change(&mut state, &first.base, "session=rotated; Path=/");
    drop(first.durable(state).await.unwrap());
    for (source, builtin, url, seed) in [
        ("two", "musopia", base, "session=seed"),
        ("one", "another-definition", base, "session=seed"),
        ("one", "musopia", "https://other.example/", "session=seed"),
        (
            "one",
            "musopia",
            "https://tracker.example:444/",
            "session=seed",
        ),
        ("one", "musopia", "http://tracker.example/", "session=seed"),
        (
            "one",
            "musopia",
            "https://tracker.example/other/",
            "session=seed",
        ),
        ("one", "musopia", base, "session=replacement"),
    ] {
        let other = bound(&repository, source, builtin, url, seed);
        assert!(!Arc::ptr_eq(&first.state, &other.state));
        let mut state = other
            .durable(other.state.clone().lock_owned().await)
            .await
            .unwrap();
        assert_eq!(header(&mut state, &other.base), seed);
    }
    let mut state = second.state.lock().await;
    assert_eq!(header(&mut state, &second.base), "session=rotated");
    drop(state);
    drop(first);
    drop(second);
    let restarted = bound(
        &SessionRepository::new(&dir.path().join("config.json")),
        "one",
        "musopia",
        base,
        "session=seed",
    );
    let mut state = restarted
        .durable(restarted.state.clone().lock_owned().await)
        .await
        .unwrap();
    assert_eq!(header(&mut state, &restarted.base), "session=rotated");
}

#[tokio::test]
async fn cancelled_save_keeps_the_writer_until_rename_then_next_request_uses_new_cookie() {
    let dir = tempfile::tempdir().unwrap();
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    async fn respond(
        State(seen): State<Arc<std::sync::Mutex<Vec<String>>>>,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        seen.lock()
            .unwrap()
            .push(headers.get(COOKIE).unwrap().to_str().unwrap().into());
        axum::response::Html("<a href='logout.php'>Logout</a>")
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new()
        .fallback(get(respond))
        .with_state(seen.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let repository = SessionRepository::new(&dir.path().join("config.json"));
    let first = Arc::new(bound(&repository, "one", "musopia", &base, "session=seed"));
    let second = Arc::new(bound(&repository, "one", "musopia", &base, "session=seed"));
    let mut state = first
        .durable(first.state.clone().lock_owned().await)
        .await
        .unwrap();
    change(&mut state, &first.base, "session=old; Path=/");
    let mut state = first.durable(state).await.unwrap();
    let file = state.file.as_ref().unwrap().clone();
    let (ready, release) = file.pause_next_write();
    change(&mut state, &first.base, "session=new; Path=/");
    let worker = {
        let session = first.clone();
        tokio::spawn(async move {
            drop(session.durable(state).await.unwrap());
        })
    };
    tokio::time::timeout(Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    assert!(second.state.try_lock().is_err());
    let old = file
        .load(&first.base)
        .unwrap()
        .unwrap()
        .header(&first.base, chrono::Utc::now().timestamp())
        .unwrap()
        .unwrap();
    assert_eq!(old, "session=old");
    assert!(seen.lock().unwrap().is_empty());
    let next = {
        let session = second.clone();
        tokio::spawn(async move { session.test().await })
    };
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), next)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(*seen.lock().unwrap(), vec!["session=new"]);
    let mut saved = file.load(&first.base).unwrap().unwrap();
    assert_eq!(
        saved
            .header(&first.base, chrono::Utc::now().timestamp())
            .unwrap()
            .unwrap(),
        "session=new"
    );
    assert!(!first.state.lock().await.dirty);
    server.abort();
}

#[tokio::test]
async fn deletion_is_a_durable_empty_snapshot_and_failed_save_stays_dirty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let repository = SessionRepository::new(&path);
    let session = bound(
        &repository,
        "one",
        "musopia",
        "https://tracker.example/",
        "session=seed",
    );
    let mut state = session
        .durable(session.state.clone().lock_owned().await)
        .await
        .unwrap();
    change(
        &mut state,
        &session.base,
        "session=deleted; Max-Age=0; Path=/",
    );
    // Fail the save after the missing-file load succeeded, without any HTTP retry.
    std::fs::write(&repository.directory, b"blocked-storage").unwrap();
    let error = match session.durable(state).await {
        Ok(_) => panic!("save unexpectedly succeeded"),
        Err(error) => error,
    };
    assert_eq!(
        error.to_string(),
        "Unable to persist refreshed cookie session; retry after restoring storage"
    );
    assert!(session.state.lock().await.dirty);
    std::fs::remove_file(&repository.directory).unwrap();
    let mut state = session
        .durable(session.state.clone().lock_owned().await)
        .await
        .unwrap();
    assert!(!state.dirty);
    assert_eq!(header(&mut state, &session.base), "");
    drop(state);
    drop(session);
    let restarted = bound(
        &SessionRepository::new(&path),
        "one",
        "musopia",
        "https://tracker.example/",
        "session=seed",
    );
    let mut state = restarted
        .durable(restarted.state.clone().lock_owned().await)
        .await
        .unwrap();
    assert_eq!(header(&mut state, &restarted.base), "");
}

#[tokio::test]
async fn failed_flush_keeps_the_applied_cookie_dirty_until_durability_is_confirmed() {
    let dir = tempfile::tempdir().unwrap();
    let repository = SessionRepository::new(&dir.path().join("config.json"));
    let session = bound(
        &repository,
        "one",
        "musopia",
        "https://tracker.example/",
        "session=seed",
    );
    let mut state = session
        .durable(session.state.clone().lock_owned().await)
        .await
        .unwrap();
    change(&mut state, &session.base, "session=rotated; Path=/");
    let file = state.file.as_ref().unwrap().clone();
    file.fail_next_flush();
    assert!(session.durable(state).await.is_err());
    let mut state = session.state.clone().lock_owned().await;
    assert!(state.dirty);
    assert_eq!(header(&mut state, &session.base), "session=rotated");
    assert_eq!(
        file.load(&session.base)
            .unwrap()
            .unwrap()
            .header(&session.base, 0)
            .unwrap()
            .unwrap(),
        "session=rotated"
    );
    let state = session.durable(state).await.unwrap();
    assert!(!state.dirty);
}

#[tokio::test]
async fn failed_response_save_stops_network_until_storage_recovers() {
    struct Fixture {
        directory: PathBuf,
        seen: std::sync::Mutex<Vec<String>>,
    }
    async fn respond(
        State(fixture): State<Arc<Fixture>>,
        headers: HeaderMap,
    ) -> axum::response::Response {
        let mut seen = fixture.seen.lock().unwrap();
        seen.push(headers.get(COOKIE).unwrap().to_str().unwrap().to_owned());
        let mut response = axum::response::Html("<a href='logout.php'>Logout</a>").into_response();
        if seen.len() == 1 {
            std::fs::write(&fixture.directory, b"blocked-storage").unwrap();
            response
                .headers_mut()
                .insert(SET_COOKIE, "session=rotated; Path=/".parse().unwrap());
        }
        response
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let repository = SessionRepository::new(&path);
    let fixture = Arc::new(Fixture {
        directory: repository.directory.clone(),
        seen: Default::default(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new()
        .fallback(get(respond))
        .with_state(fixture.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let session = bound(&repository, "one", "musopia", &base, "session=seed");
    for _ in 0..2 {
        assert_eq!(
            session.test().await.unwrap_err().to_string(),
            "Unable to persist refreshed cookie session; retry after restoring storage"
        );
        assert_eq!(*fixture.seen.lock().unwrap(), vec!["session=seed"]);
    }
    std::fs::remove_file(&fixture.directory).unwrap();
    session.test().await.unwrap();
    assert_eq!(
        *fixture.seen.lock().unwrap(),
        vec!["session=seed", "session=rotated"]
    );
    drop(session);
    let restarted = bound(
        &SessionRepository::new(&path),
        "one",
        "musopia",
        &base,
        "session=seed",
    );
    restarted.test().await.unwrap();
    assert_eq!(
        *fixture.seen.lock().unwrap(),
        vec!["session=seed", "session=rotated", "session=rotated"]
    );
    server.abort();
}

#[test]
fn search_header_contract_cannot_override_cookie_user_agent() {
    let node = crate::cardigann::parse(
        include_str!("../../vendor/jackett/definitions/hdtime.yml"),
        crate::cardigann::Limits::default(),
    )
    .unwrap();
    let session = CookieSession::new(
        CookiePlan::compile(&node)
            .unwrap()
            .with_user_agent(Some("Fixture Browser/1.0"))
            .unwrap(),
        "https://tracker.example/",
        "session=fixture",
        0,
        2,
    )
    .unwrap();
    let agent = ("User-Agent".to_owned(), "Fixture Browser/1.0".to_owned());
    assert!(session
        .check_search_headers(std::slice::from_ref(&agent))
        .is_ok());
    assert!(session
        .check_search_headers(&[("user-agent".into(), agent.1.clone())])
        .is_ok());
    for headers in [
        vec![],
        vec![("User-Agent".into(), "other".into())],
        vec![agent.clone(), agent.clone()],
        vec![agent.clone(), ("Referer".into(), "secret".into())],
    ] {
        assert_eq!(
            session
                .check_search_headers(&headers)
                .unwrap_err()
                .to_string(),
            "Unsupported authenticated search headers"
        );
    }
    let node = crate::cardigann::parse(
        include_str!("../../vendor/jackett/definitions/musopia.yml"),
        crate::cardigann::Limits::default(),
    )
    .unwrap();
    let ordinary = CookieSession::new(
        CookiePlan::compile(&node).unwrap(),
        "https://tracker.example/",
        "session=fixture",
        0,
        2,
    )
    .unwrap();
    assert!(ordinary.check_search_headers(&[]).is_ok());
    assert!(ordinary.check_search_headers(&[agent]).is_err());
}

#[tokio::test]
async fn changed_user_agent_cannot_reuse_durable_browser_cookies() {
    let make = |repo: &SessionRepository, agent: &str| {
        let node = crate::cardigann::parse(
            include_str!("../../vendor/jackett/definitions/hdtime.yml"),
            crate::cardigann::Limits::default(),
        )
        .unwrap();
        let mut session = CookieSession::new(
            CookiePlan::compile(&node)
                .unwrap()
                .with_user_agent(Some(agent))
                .unwrap(),
            "https://tracker.example/",
            "session=seed",
            0,
            2,
        )
        .unwrap();
        session.bind(repo, "one", "hdtime").unwrap();
        session
    };
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.json");
    let repo = SessionRepository::new(&config);
    let first = make(&repo, "Fixture Browser/1.0");
    let mut state = first
        .durable(first.state.clone().lock_owned().await)
        .await
        .unwrap();
    change(&mut state, &first.base, "session=renewed; Path=/");
    drop(first.durable(state).await.unwrap());
    drop(first);
    drop(repo);
    let repo = SessionRepository::new(&config);
    let same = make(&repo, "Fixture Browser/1.0");
    let mut state = same
        .durable(same.state.clone().lock_owned().await)
        .await
        .unwrap();
    assert_eq!(header(&mut state, &same.base), "session=renewed");
    drop(state);
    let changed = make(&repo, "Fixture Browser/2.0");
    assert!(!Arc::ptr_eq(&same.state, &changed.state));
    let mut state = changed
        .durable(changed.state.clone().lock_owned().await)
        .await
        .unwrap();
    assert_eq!(header(&mut state, &changed.base), "session=seed");
}

/// Loopback source without `login.test`: mode 0 serves a page with no login marker,
/// 1 redirects the next search once, 2 always redirects searches, 3 answers 401.
struct NoTestSource {
    hits: std::sync::Mutex<Vec<String>>,
    mode: std::sync::atomic::AtomicU8,
    posts: std::sync::atomic::AtomicUsize,
}
impl NoTestSource {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.hits.lock().unwrap())
    }
}
async fn no_test_source(
    State(source): State<Arc<NoTestSource>>,
    method: axum::http::Method,
    uri: axum::http::Uri,
) -> axum::response::Response {
    use std::sync::atomic::Ordering;
    source
        .hits
        .lock()
        .unwrap()
        .push(format!("{method} {}", uri.path()));
    let redirect = || {
        let mut r = axum::http::StatusCode::FOUND.into_response();
        r.headers_mut()
            .insert(LOCATION, "/addon.php?page=visitor-test".parse().unwrap());
        r
    };
    match (method.as_str(), uri.path()) {
        ("GET", "/addon.php") => axum::response::Html(
            "<form id=visitor-test-form action='addon.php?r=submit'>\
             <input name=visitor_test value=robot></form>",
        )
        .into_response(),
        ("POST", "/addon.php") => {
            source.posts.fetch_add(1, Ordering::SeqCst);
            let mut r = axum::response::Html("<p>Welcome</p>").into_response();
            r.headers_mut()
                .insert(SET_COOKIE, "visitor=human; Path=/".parse().unwrap());
            r
        }
        _ => match source.mode.load(Ordering::SeqCst) {
            1 => {
                source.mode.store(0, Ordering::SeqCst);
                redirect()
            }
            2 => redirect(),
            3 => axum::http::StatusCode::UNAUTHORIZED.into_response(),
            _ => axum::response::Html("<p>Nothing that looks like a login marker</p>")
                .into_response(),
        },
    }
}
async fn serve_no_test() -> (Arc<NoTestSource>, String, tokio::task::JoinHandle<()>) {
    let source = Arc::new(NoTestSource {
        hits: Default::default(),
        mode: Default::default(),
        posts: Default::default(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new()
        .fallback(axum::routing::any(no_test_source))
        .with_state(source.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (source, base, server)
}

#[tokio::test]
async fn cookie_login_without_test_sends_no_validation_and_rejects_search_redirects() {
    let node = crate::cardigann::parse(
        include_str!("../../vendor/jackett/definitions/rousi-pro.yml"),
        crate::cardigann::Limits::default(),
    )
    .unwrap();
    let (source, base, server) = serve_no_test().await;
    let session = CookieSession::new(
        CookiePlan::compile(&node).unwrap(),
        &base,
        "session=seed",
        0,
        5,
    )
    .unwrap();
    session.test().await.unwrap();
    assert!(source.take().is_empty(), "pinned TestLogin sends nothing");
    // A page without any login marker is returned as-is, never treated as expiry.
    let body = session.fetch_search("search", None, true).await.unwrap();
    assert!(body.contains("Nothing that looks like a login marker"));
    assert_eq!(source.take(), vec!["GET /search"]);
    // A search redirect is still expiry; supplied cookies cannot reauthenticate,
    // so the search fails without following it.
    source.mode.store(2, std::sync::atomic::Ordering::SeqCst);
    assert!(session.fetch_search("search", None, true).await.is_err());
    assert_eq!(source.take(), vec!["GET /search"]);
    server.abort();
}

#[tokio::test]
async fn form_login_without_test_relogs_only_after_a_search_redirect() {
    use super::super::form_auth::FormPlan;
    use std::sync::atomic::Ordering;
    let node = crate::cardigann::parse(
        include_str!("../../vendor/jackett/definitions/miobt.yml"),
        crate::cardigann::Limits::default(),
    )
    .unwrap();
    let (source, base, server) = serve_no_test().await;
    let session = CookieSession::new_form(
        FormPlan::compile(&node).unwrap(),
        &base,
        &Default::default(),
        0,
        5,
    )
    .unwrap();
    let login = ["GET /addon.php", "POST /addon.php"];
    let page = session.fetch_search("search", None, false).await.unwrap();
    assert!(page.contains("Nothing that looks like a login marker"));
    // Login, then the search: no validation request (`CardigannIndexer.cs:896-899`).
    assert_eq!(source.take(), [&login[..], &["GET /search"]].concat());
    // The stored session is trusted; a marker-less page does not trigger relogin.
    session.fetch_search("search", None, false).await.unwrap();
    session.test().await.unwrap();
    assert_eq!(source.take(), vec!["GET /search"]);
    assert_eq!(source.posts.load(Ordering::SeqCst), 1);
    // One search redirect: a single relogin, then the retried search.
    source.mode.store(1, Ordering::SeqCst);
    session.fetch_search("search", None, false).await.unwrap();
    assert_eq!(
        source.take(),
        [&["GET /search"], &login[..], &["GET /search"]].concat()
    );
    assert_eq!(source.posts.load(Ordering::SeqCst), 2);
    // A persistent redirect fails after exactly one relogin.
    source.mode.store(2, Ordering::SeqCst);
    assert!(session.fetch_search("search", None, false).await.is_err());
    assert_eq!(
        source.take(),
        [&["GET /search"], &login[..], &["GET /search"]].concat()
    );
    assert_eq!(source.posts.load(Ordering::SeqCst), 3);
    // Without a test selector, 401 is not a login signal: no relogin.
    source.mode.store(3, Ordering::SeqCst);
    assert!(session.fetch_search("search", None, false).await.is_err());
    assert_eq!(source.take(), vec!["GET /search"]);
    assert_eq!(source.posts.load(Ordering::SeqCst), 3);
    server.abort();
}

/// Loopback 4thd-shaped source: a request carrying the current session cookie sees
/// the login marker on every page; otherwise it is redirected to the login form.
/// `broken` serves searches without the marker even to a valid session.
struct MarkedSource {
    hits: std::sync::Mutex<Vec<String>>,
    session: std::sync::Mutex<String>,
    posts: std::sync::atomic::AtomicUsize,
    broken: std::sync::atomic::AtomicBool,
}
impl MarkedSource {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.hits.lock().unwrap())
    }
    fn expire(&self) {
        self.session.lock().unwrap().clear();
    }
}
async fn marked_source(
    State(source): State<Arc<MarkedSource>>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
) -> axum::response::Response {
    use std::sync::atomic::Ordering;
    source
        .hits
        .lock()
        .unwrap()
        .push(format!("{method} {}", uri.path()));
    let mut session = source.session.lock().unwrap();
    match (method.as_str(), uri.path()) {
        ("GET", "/login.php") => axum::response::Html(
            "<form id=loginform action='login.php'><input name=username><input name=password></form>",
        )
        .into_response(),
        ("POST", "/login.php") => {
            let posts = source.posts.fetch_add(1, Ordering::SeqCst) + 1;
            *session = format!("session=login-{posts}");
            let mut r = axum::response::Html("<p>Welcome</p>").into_response();
            r.headers_mut().insert(
                SET_COOKIE,
                format!("{session}; Path=/").parse().unwrap(),
            );
            r
        }
        (_, path) => {
            let cookie = headers.get(COOKIE).and_then(|v| v.to_str().ok());
            if session.is_empty() || cookie != Some(session.as_str()) {
                let mut r = axum::http::StatusCode::FOUND.into_response();
                r.headers_mut()
                    .insert(LOCATION, "/login.php".parse().unwrap());
                return r;
            }
            if path == "/torrents.php" && source.broken.load(Ordering::SeqCst) {
                return axum::response::Html("<p>Maintenance</p>").into_response();
            }
            axum::response::Html("<a href='logout.php?auth=fixture'>Logout</a>").into_response()
        }
    }
}
async fn serve_marked(session: &str) -> (Arc<MarkedSource>, String, tokio::task::JoinHandle<()>) {
    let source = Arc::new(MarkedSource {
        hits: Default::default(),
        session: std::sync::Mutex::new(session.into()),
        posts: Default::default(),
        broken: Default::default(),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new()
        .fallback(axum::routing::any(marked_source))
        .with_state(source.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (source, base, server)
}
fn form_4thd(repository: &SessionRepository, base: &str) -> CookieSession {
    let node = crate::cardigann::parse(
        include_str!("../../vendor/jackett/definitions/4thd.yml"),
        crate::cardigann::Limits::default(),
    )
    .unwrap();
    let settings = std::collections::BTreeMap::from([
        ("username".into(), serde_json::json!("fixture-user")),
        ("password".into(), serde_json::json!("fixture-password")),
    ]);
    let mut session = CookieSession::new_form(
        super::super::form_auth::FormPlan::compile(&node).unwrap(),
        base,
        &settings,
        0,
        5,
    )
    .unwrap();
    session.bind(repository, "one", "4thd").unwrap();
    session
}
const LOGIN: [&str; 3] = ["GET /login.php", "POST /login.php", "GET /index.php"];
const SEARCH: &str = "GET /torrents.php";

#[tokio::test]
async fn validated_form_session_sends_no_test_before_later_search_pages() {
    let dir = tempfile::tempdir().unwrap();
    let (source, base, server) = serve_marked("").await;
    let session = form_4thd(
        &SessionRepository::new(&dir.path().join("config.json")),
        &base,
    );
    session
        .fetch_search("torrents.php", None, false)
        .await
        .unwrap();
    // Login and TestLogin once (pinned ApplyConfiguration, `CardigannIndexer.cs:1106-1107`).
    assert_eq!(source.take(), [&LOGIN[..], &[SEARCH]].concat());
    // Later pages and searches trust the validated session, as pinned PerformQuery
    // does (`CardigannIndexer.cs:1517-1625`): one request per page.
    for _ in 0..3 {
        session
            .fetch_search("torrents.php", None, false)
            .await
            .unwrap();
    }
    assert_eq!(source.take(), vec![SEARCH; 3]);
    // An explicit test still checks the session.
    session.test().await.unwrap();
    assert_eq!(source.take(), vec!["GET /index.php"]);
    assert_eq!(source.posts.load(std::sync::atomic::Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn search_page_expiry_relogs_exactly_once_then_retries() {
    use std::sync::atomic::Ordering;
    let dir = tempfile::tempdir().unwrap();
    let (source, base, server) = serve_marked("").await;
    let session = form_4thd(
        &SessionRepository::new(&dir.path().join("config.json")),
        &base,
    );
    session
        .fetch_search("torrents.php", None, false)
        .await
        .unwrap();
    source.take();
    // The search response itself shows expiry (pinned CheckIfLoginIsNeeded and
    // Relogin, `CardigannIndexer.cs:874-890`, `:951-997`).
    source.expire();
    session
        .fetch_search("torrents.php", None, false)
        .await
        .unwrap();
    assert_eq!(source.take(), [&[SEARCH], &LOGIN[..], &[SEARCH]].concat());
    assert_eq!(source.posts.load(Ordering::SeqCst), 2);
    session
        .fetch_search("torrents.php", None, false)
        .await
        .unwrap();
    assert_eq!(source.take(), vec![SEARCH]);
    // A page still missing the marker after the relogin fails without a second
    // relogin and leaves the session unvalidated: the next search tests it first.
    source.broken.store(true, Ordering::SeqCst);
    assert!(session
        .fetch_search("torrents.php", None, false)
        .await
        .is_err());
    assert_eq!(source.take(), [&[SEARCH], &LOGIN[..], &[SEARCH]].concat());
    assert_eq!(source.posts.load(Ordering::SeqCst), 3);
    source.broken.store(false, Ordering::SeqCst);
    session
        .fetch_search("torrents.php", None, false)
        .await
        .unwrap();
    assert_eq!(source.take(), vec!["GET /index.php", SEARCH]);
    assert_eq!(source.posts.load(Ordering::SeqCst), 3);
    server.abort();
}

#[tokio::test]
async fn restart_and_identity_change_validate_the_stored_session_once() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    let (source, base, server) = serve_marked("").await;
    let repository = SessionRepository::new(&config);
    let first = form_4thd(&repository, &base);
    first
        .fetch_search("torrents.php", None, false)
        .await
        .unwrap();
    source.take();
    // A runtime replacement with the same identity shares the validated state.
    let edited = form_4thd(&repository, &base);
    edited
        .fetch_search("torrents.php", None, false)
        .await
        .unwrap();
    assert_eq!(source.take(), vec![SEARCH]);
    drop((first, edited, repository));
    // Validation is in memory only: the restored session is tested once, not
    // logged into again, then trusted.
    let restarted = form_4thd(&SessionRepository::new(&config), &base);
    for _ in 0..2 {
        restarted
            .fetch_search("torrents.php", None, false)
            .await
            .unwrap();
    }
    assert_eq!(source.take(), vec!["GET /index.php", SEARCH, SEARCH]);
    assert_eq!(source.posts.load(std::sync::atomic::Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn supplied_cookie_session_validates_once_and_fails_visibly_on_expiry() {
    let (source, base, server) = serve_marked("session=seed").await;
    let session = CookieSession::new(
        CookiePlan {
            aidoru_fragment: false,
            user_agent: None,
            declared: Vec::new(),
            header_settings: Default::default(),
            headers: Vec::new(),
            charset: crate::cardigann_exec::request::Charset::Utf8,
            certificates: None,
            test: Some(LoginTest {
                path: "index.php".into(),
                selector: Some("a[href^='logout.php']".into()),
            }),
        },
        &base,
        "session=seed",
        0,
        5,
    )
    .unwrap();
    for _ in 0..3 {
        session
            .fetch_search("torrents.php", None, false)
            .await
            .unwrap();
    }
    assert_eq!(
        source.take(),
        vec!["GET /index.php", SEARCH, SEARCH, SEARCH]
    );
    // Supplied cookies cannot reauthenticate: the expired search fails, and so does
    // the next one, at its test request.
    source.expire();
    assert!(session
        .fetch_search("torrents.php", None, false)
        .await
        .is_err());
    assert_eq!(source.take(), vec![SEARCH]);
    assert!(session
        .fetch_search("torrents.php", None, false)
        .await
        .is_err());
    assert_eq!(source.take(), vec!["GET /index.php", "GET /login.php"]);
    assert_eq!(source.posts.load(std::sync::atomic::Ordering::SeqCst), 0);
    server.abort();
}

/// Synthetic cookie source declaring every admitted search-header shape.
const HEADER_SOURCE: &str = r#"id: fixture
name: Fixture
description: Synthetic fixture
language: en-US
type: private
encoding: UTF-8
links: [https://example.invalid/]
caps: {categories: {1: Other}, modes: {search: [q]}}
settings:
  - {name: cookie, type: text, label: Cookie}
  - {name: useragent, type: text, label: User-Agent}
  - {name: csrf_token, type: text, label: Token}
login:
  method: cookie
  inputs: {cookie: "{{ .Config.cookie }}"}
  test: {path: index.php, selector: "a[href^='logout.php']"}
search:
  headers:
    User-Agent: ["{{ .Config.useragent }}"]
    Referer: ["{{ .Config.sitelink }}browse.php"]
    Cookie: ["layout=classic; lang=en"]
    x-csrf-token: ["{{ .Config.csrf_token }}"]
  paths: [{path: torrents.php}]
  rows: {selector: tr}
  fields: {title: {selector: a}}
"#;
fn header_plan(yaml: &str) -> Result<CookiePlan> {
    CookiePlan::compile(
        &crate::cardigann::parse(yaml, crate::cardigann::Limits::default()).unwrap(),
    )
}
fn header_session(base: &str, token: Option<&str>) -> Result<CookieSession> {
    let mut settings = serde_json::Map::new();
    if let Some(token) = token {
        settings.insert("csrf_token".into(), token.into());
    }
    CookieSession::new(
        header_plan(HEADER_SOURCE)?
            .with_user_agent(Some("Fixture Browser/1.0"))?
            .with_settings(&settings)?,
        base,
        "session=seed",
        0,
        5,
    )
}
/// One recorded request: method and path, then the headers the tests inspect.
#[derive(Debug, PartialEq)]
struct Seen {
    request: String,
    agent: Option<String>,
    referer: Option<String>,
    cookie: Option<String>,
    csrf: Option<String>,
}
async fn recording_source(
    State(seen): State<Arc<std::sync::Mutex<Vec<Seen>>>>,
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: HeaderMap,
) -> axum::response::Response {
    let get = |name: &str| headers.get(name).map(|v| v.to_str().unwrap().to_owned());
    seen.lock().unwrap().push(Seen {
        request: format!("{method} {}", uri.path()),
        agent: get("user-agent"),
        referer: get("referer"),
        cookie: get("cookie"),
        csrf: get("x-csrf-token"),
    });
    match (method.as_str(), uri.path()) {
        ("GET", "/dl.php") => {
            let mut r = axum::http::StatusCode::FOUND.into_response();
            r.headers_mut()
                .insert(LOCATION, "/file.torrent".parse().unwrap());
            r
        }
        ("GET", "/file.torrent") => b"d4:infod4:name7:fixtureee".to_vec().into_response(),
        ("GET", "/login.php") => axum::response::Html(
            "<form id=loginform action='login.php'><input name=username><input name=password></form>",
        )
        .into_response(),
        ("POST", "/login.php") => {
            let mut r = axum::response::Html("<p>Welcome</p>").into_response();
            r.headers_mut()
                .insert(SET_COOKIE, "session=login; Path=/".parse().unwrap());
            r
        }
        _ => axum::response::Html("<a href='logout.php?auth=fixture'>Logout</a>").into_response(),
    }
}
async fn serve_recording() -> (
    Arc<std::sync::Mutex<Vec<Seen>>>,
    String,
    tokio::task::JoinHandle<()>,
) {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let app = Router::new()
        .fallback(axum::routing::any(recording_source))
        .with_state(seen.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (seen, base, server)
}
fn seen(
    request: &str,
    agent: &str,
    referer: Option<String>,
    cookie: &str,
    csrf: Option<&str>,
) -> Seen {
    Seen {
        request: request.into(),
        agent: Some(agent.into()),
        referer,
        cookie: Some(cookie.into()),
        csrf: csrf.map(Into::into),
    }
}

#[tokio::test]
async fn declared_headers_go_with_test_search_and_download_first_requests() {
    let (recorded, base, server) = serve_recording().await;
    let session = header_session(&base, Some("token-1")).unwrap();
    let merged = "layout=classic; lang=en; session=seed";
    let agent = "Fixture Browser/1.0";
    let browse = Some(format!("{base}browse.php"));
    session
        .fetch_search("torrents.php", None, false)
        .await
        .unwrap();
    let bytes = session.fetch_download("dl.php").await.unwrap();
    assert!(bytes.starts_with(b"d4:info"));
    assert_eq!(
        std::mem::take(&mut *recorded.lock().unwrap()),
        [
            // TestLogin and PerformQuery: `CardigannIndexer.cs:903-904`, `:1605-1607`.
            seen(
                "GET /index.php",
                agent,
                browse.clone(),
                merged,
                Some("token-1")
            ),
            seen(
                "GET /torrents.php",
                agent,
                browse.clone(),
                merged,
                Some("token-1")
            ),
            // Downloads validate the session first (docs/session-validation.md).
            seen("GET /index.php", agent, browse, merged, Some("token-1")),
            // BaseIndexer.Download: the link is the Referer (`BaseIndexer.cs:537-548`).
            seen(
                "GET /dl.php",
                agent,
                Some(format!("{base}dl.php")),
                merged,
                Some("token-1")
            ),
            // FollowIfRedirect builds the hop without custom headers or Referer.
            seen("GET /file.torrent", agent, None, "session=seed", None),
        ]
    );
    server.abort();
}

#[test]
fn literal_cookie_header_precedes_the_session_jar_and_stands_alone_without_one() {
    let session = header_session("https://tracker.example/", Some("token")).unwrap();
    assert_eq!(
        session.cookie_header(true, Some("session=seed".into())),
        Some("layout=classic; lang=en; session=seed".into())
    );
    assert_eq!(
        session.cookie_header(true, None),
        Some("layout=classic; lang=en".into())
    );
    // Redirect hops carry only the jar.
    assert_eq!(
        session.cookie_header(false, Some("session=seed".into())),
        Some("session=seed".into())
    );
    assert_eq!(session.cookie_header(false, None), None);
}

#[test]
fn header_bound_values_are_session_identity_and_never_echoed() {
    let base = "https://tracker.example/";
    let one = header_session(base, Some("token-1")).unwrap();
    assert_eq!(
        one.seed_identity,
        header_session(base, Some("token-1")).unwrap().seed_identity
    );
    let two = header_session(base, Some("token-2")).unwrap();
    assert_ne!(one.seed_identity, two.seed_identity);
    let mirror = header_session("https://mirror.example/", Some("token-1")).unwrap();
    assert_ne!(one.seed_identity, mirror.seed_identity, "site-link Referer");
    // A source without declared headers keeps its pre-existing identity.
    let plain = CookieSession::new(
        header_plan(
            &HEADER_SOURCE
                .lines()
                .filter(|l| *l != "  headers:" && !l.starts_with("    "))
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap(),
        base,
        "session=seed",
        0,
        5,
    )
    .unwrap();
    assert_eq!(
        plain.seed_identity,
        <[u8; 32]>::from(Sha256::digest(b"session=seed"))
    );
    for token in [
        None,
        Some(""),
        Some(" "),
        Some("t\u{e9}"),
        Some("a\r\nHost: evil"),
    ] {
        let error = header_session(base, token).err().unwrap().to_string();
        assert_eq!(error, "This preset requires a valid csrf_token setting");
    }
    // The plan's rendered set is accepted in any order and header-name case.
    let expected = [
        ("x-csrf-token".to_owned(), "token-1".to_owned()),
        ("cookie".into(), "layout=classic; lang=en".into()),
        ("REFERER".into(), format!("{base}browse.php")),
        ("User-Agent".into(), "Fixture Browser/1.0".into()),
    ];
    one.check_search_headers(&expected).unwrap();
    for headers in [
        expected[1..].to_vec(),
        [&expected[..], &expected[..1]].concat(),
        vec![
            ("x-csrf-token".into(), "token-2".into()),
            expected[1].clone(),
            expected[2].clone(),
            expected[3].clone(),
        ],
    ] {
        let error = one.check_search_headers(&headers).unwrap_err().to_string();
        assert_eq!(error, "Unsupported authenticated search headers");
    }
}

#[test]
fn session_header_declarations_admit_only_modeled_shapes() {
    let line = "    x-csrf-token: [\"{{ .Config.csrf_token }}\"]";
    assert!(HEADER_SOURCE.contains(line));
    assert!(header_plan(HEADER_SOURCE).is_ok());
    for replacement in [
        "    Referer: [https://foreign.invalid/]",
        "    Referer: [\"browse.php{{ .Config.sitelink }}\"]",
        "    Referer: [\"{{ .Config.sitelink }}{{ .Config.csrf_token }}\"]",
        "    Cookie: [\"{{ .Config.csrf_token }}\"]",
        "    Cookie: [\"a=b; a=c\"]",
        "    Cookie: [\"a b\"]",
        "    Host: [example.invalid]",
        "    Content-Type: [text/plain]",
        "    Transfer-Encoding: [chunked]",
        "    referer: [\"{{ .Config.sitelink }}\"]",
        "    X-Two: [a, b]",
        "    X-None: []",
        "    X-Query: [\"{{ .Keywords }}\"]",
        "    X-Filter: [\"{{ .Config.csrf_token | lower }}\"]",
        "    X-Cookie: [\"{{ .Config.cookie }}\"]",
        "    X-Agent: [\"{{ .Config.useragent }}\"]",
        "    X-Undeclared: [\"{{ .Config.missing }}\"]",
        "    X-Open: [\"a {{ b\"]",
        "    X-Close: [\"a }} b\"]",
        "    X-Control: [\"a\\tb\"]",
        "    \"Bad Name\": [x]",
    ] {
        let yaml = HEADER_SOURCE.replace(line, &format!("{line}\n{replacement}"));
        assert!(header_plan(&yaml).is_err(), "{replacement}");
    }
    let password = HEADER_SOURCE.replace(
        "{name: csrf_token, type: text",
        "{name: csrf_token, type: password",
    );
    assert!(header_plan(&password).is_err(), "only text settings");
}

#[tokio::test]
async fn form_sessions_send_declared_headers_with_login_and_search_requests() {
    let (recorded, base, server) = serve_recording().await;
    let yaml = include_str!("../../vendor/jackett/definitions/4thd.yml").replace(
        "search:\n  paths:",
        "search:\n  headers:\n    Referer: [\"{{ .Config.sitelink }}torrents.php\"]\n    Cookie: [lang=en]\n    x-csrf-token: [\"{{ .Config.username }}\"]\n  paths:",
    );
    let node = crate::cardigann::parse(&yaml, crate::cardigann::Limits::default()).unwrap();
    let settings = std::collections::BTreeMap::from([
        ("username".into(), serde_json::json!("fixture-user")),
        ("password".into(), serde_json::json!("fixture-password")),
    ]);
    let session = CookieSession::new_form(
        super::super::form_auth::FormPlan::compile(&node).unwrap(),
        &base,
        &settings,
        0,
        5,
    )
    .unwrap();
    session
        .fetch_search("torrents.php", None, false)
        .await
        .unwrap();
    let agent = "Rackett/0.1";
    let declared = Some(format!("{base}torrents.php"));
    let user = Some("fixture-user");
    assert_eq!(
        std::mem::take(&mut *recorded.lock().unwrap()),
        [
            // GetConfigurationForSetup: Referer is SiteLink (`CardigannIndexer.cs:1021-1029`).
            seen("GET /login.php", agent, Some(base.clone()), "lang=en", user),
            // Form POST: Referer is the login page (`CardigannIndexer.cs:804`).
            seen(
                "POST /login.php",
                agent,
                Some(format!("{base}login.php")),
                "lang=en",
                user
            ),
            seen(
                "GET /index.php",
                agent,
                declared.clone(),
                "lang=en; session=login",
                user
            ),
            seen(
                "GET /torrents.php",
                agent,
                declared,
                "lang=en; session=login",
                user
            ),
        ]
    );
    // A form source binds no User-Agent setting.
    let agent_yaml = yaml.replace(
        "    Cookie: [lang=en]",
        "    User-Agent: [\"{{ .Config.username }}\"]",
    );
    let node = crate::cardigann::parse(&agent_yaml, crate::cardigann::Limits::default()).unwrap();
    assert!(super::super::form_auth::FormPlan::compile(&node).is_err());
    server.abort();
}
