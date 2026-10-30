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
            charset: crate::cardigann_exec::request::Charset::Utf8,
            certificates: None,
            path: "index.php".into(),
            selector: "a[href='logout.php']".into(),
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
