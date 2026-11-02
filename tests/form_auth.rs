//! Synthetic loopback evidence for the shared form transport, not provider admission.
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::any,
    Router,
};
use rackett::{
    builtin_runtime::{auth::CookieSession, form_auth::FormPlan},
    cardigann,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

const YAML: &str = r#"
encoding: UTF-8
settings:
  - {name: username, type: text}
  - {name: password, type: password}
login:
  method: form
  path: login
  form: form#login
  inputs:
    username: '{{ .Config.username }}'
    password: '{{ .Config.password }}'
    keep: 'yes'
  test: {path: account, selector: 'a[href="logout"]'}
  error:
    - selector: '.login-error'
      message: {text: 'never expose this message'}
search:
  paths: [{path: search}]
"#;
const FORM: &str = r#"<form id=login action=submit>
<input name=csrf value='fresh &amp; token'><input name=username value=old>
<input name=dup value=first><input name=dup value=last>
<input name=disabled value=no disabled><input type=checkbox name=unchecked value=no>
<input type=checkbox name=checked value=yes checked><input type=checkbox name=empty checked>
<fieldset disabled><legend><input name=legend value=yes></legend>
<input name=fieldset value=no></fieldset>
<input type=radio name=radio value=first checked><input type=radio name=radio value=last checked>
<input type=radio name=disabled_radio value=first checked><input type=radio name=disabled_radio value=last checked disabled>
<template><input name=inert value=no></template><input name=keep value=no>
</form>"#;
const LOGGED: &str = "<a href=logout>Logout</a><table id=results></table>";
const USER: &str = "user & café";
const PASS: &str = "password+secret=fixture";

fn plan(yaml: &str) -> anyhow::Result<FormPlan> {
    FormPlan::compile(&cardigann::parse(yaml, cardigann::Limits::default()).unwrap())
}
fn settings() -> BTreeMap<String, Value> {
    BTreeMap::from([
        ("username".into(), json!(USER)),
        ("password".into(), json!(PASS)),
    ])
}
fn session(base: &str, yaml: &str) -> CookieSession {
    CookieSession::new_form(plan(yaml).unwrap(), base, &settings(), 0, 3).unwrap()
}
#[derive(Clone)]
struct Hit {
    method: Method,
    path: String,
    headers: HeaderMap,
    body: String,
}
struct Site {
    hits: Vec<Hit>,
    landing: String,
    logged: bool,
    posts: usize,
    post_status: StatusCode,
    post_location: String,
    post_body: String,
    account_body: Option<String>,
    account_status: StatusCode,
    search_failures: usize,
    search_unauthorized: bool,
    search_redirect: Option<String>,
    landing_challenge: bool,
    search_challenge: bool,
    refresh: bool,
    entered: Option<Arc<tokio::sync::Notify>>,
    release: Option<Arc<tokio::sync::Notify>>,
}
impl Default for Site {
    fn default() -> Self {
        Self {
            hits: vec![],
            landing: FORM.into(),
            logged: false,
            posts: 0,
            post_status: StatusCode::SEE_OTHER,
            post_location: "/welcome".into(),
            post_body: LOGGED.into(),
            account_body: None,
            account_status: StatusCode::OK,
            search_failures: 0,
            search_unauthorized: false,
            search_redirect: None,
            landing_challenge: false,
            search_challenge: false,
            refresh: false,
            entered: None,
            release: None,
        }
    }
}
fn response(status: StatusCode, body: String) -> Response {
    (status, [("content-type", "text/html; charset=UTF-8")], body).into_response()
}
async fn respond(
    State(site): State<Arc<Mutex<Site>>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (reply, pause) = {
        let mut s = site.lock().unwrap();
        s.hits.push(Hit {
            method: method.clone(),
            path: uri.path().into(),
            headers: headers.clone(),
            body: String::from_utf8(body.to_vec()).unwrap(),
        });
        let cookie = headers
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let authenticated = s.logged && cookie.split("; ").any(|v| v == "session=authenticated");
        let mut reply = match (method.as_str(), uri.path()) {
            ("GET", "/login") => {
                let mut r = response(StatusCode::OK, s.landing.clone());
                r.headers_mut()
                    .insert("set-cookie", "csrf=landing; Path=/".parse().unwrap());
                if s.landing_challenge {
                    r.headers_mut()
                        .insert("cf-mitigated", "challenge".parse().unwrap());
                }
                r
            }
            ("POST", "/submit" | "/submit-again") => {
                s.posts += 1;
                s.logged = true;
                let mut r = response(s.post_status, s.post_body.clone());
                r.headers_mut().insert(
                    "set-cookie",
                    "session=authenticated; Path=/; HttpOnly".parse().unwrap(),
                );
                if s.post_status.is_redirection() {
                    r.headers_mut()
                        .insert("location", s.post_location.parse().unwrap());
                }
                r
            }
            ("GET", "/welcome") => response(StatusCode::OK, s.post_body.clone()),
            ("GET", "/account") => response(
                s.account_status,
                s.account_body.clone().unwrap_or_else(|| {
                    if authenticated {
                        LOGGED.into()
                    } else {
                        "<form>Login</form>".into()
                    }
                }),
            ),
            ("GET", "/search") => {
                if s.search_challenge {
                    let mut r = response(StatusCode::OK, LOGGED.into());
                    r.headers_mut()
                        .insert("cf-mitigated", "challenge".parse().unwrap());
                    r
                } else if s.search_failures > 0 || !authenticated {
                    s.search_failures = s.search_failures.saturating_sub(1);
                    s.logged = false;
                    if let Some(target) = &s.search_redirect {
                        let mut r = response(StatusCode::FOUND, String::new());
                        r.headers_mut().insert("location", target.parse().unwrap());
                        return r;
                    }
                    response(
                        if s.search_unauthorized {
                            StatusCode::UNAUTHORIZED
                        } else {
                            StatusCode::OK
                        },
                        "<form>Login</form>".into(),
                    )
                } else {
                    response(StatusCode::OK, LOGGED.into())
                }
            }
            ("GET", "/download") if authenticated => {
                response(StatusCode::OK, "d4:infod4:name4:testee".into())
            }
            _ => response(StatusCode::NOT_FOUND, "unexpected route".into()),
        };
        if s.refresh {
            reply
                .headers_mut()
                .insert("refresh", "0;url=/login".parse().unwrap());
        }
        let pause = if method == Method::POST {
            s.entered.clone().zip(s.release.clone())
        } else {
            None
        };
        (reply, pause)
    };
    if let Some((entered, release)) = pause {
        entered.notify_one();
        release.notified().await;
    }
    reply
}

#[tokio::test]
async fn same_origin_expiry_redirect_relogs_once_without_following_its_target() {
    let server = serve().await;
    let session = session(&server.base, YAML);
    session.test().await.unwrap();
    {
        let mut s = server.site.lock().unwrap();
        s.search_failures = 1;
        s.search_redirect = Some("/not-the-declared-login".into());
    }
    assert_eq!(session.fetch_html("search").await.unwrap(), LOGGED);
    {
        let s = server.site.lock().unwrap();
        assert_eq!(s.posts, 2);
        assert!(!s.hits.iter().any(|h| h.path == "/not-the-declared-login"));
    }
    {
        let mut s = server.site.lock().unwrap();
        s.search_failures = 2;
    }
    assert!(session.fetch_html("search").await.is_err());
    assert_eq!(server.site.lock().unwrap().posts, 3);
    // A fresh session with a foreign expiry redirect must neither relog nor follow it.
    let foreign = serve().await;
    {
        let mut s = server.site.lock().unwrap();
        s.logged = true;
        s.search_failures = 1;
        s.search_redirect = Some(foreign.base.clone());
    }
    let posts = server.site.lock().unwrap().posts;
    hidden(session.fetch_html("search").await.unwrap_err());
    assert_eq!(server.site.lock().unwrap().posts, posts);
    assert!(foreign.site.lock().unwrap().hits.is_empty());
}

#[tokio::test]
async fn select_credential_defaults_and_explicit_options_are_validated_before_post() {
    let yaml=YAML.replace("  - {name: password, type: password}", "  - {name: password, type: password}\n  - {name: question, type: select, default: '0', options: {'0': None, '1': First}}")
        .replace("    keep: 'yes'", "    keep: 'yes'\n    question: '{{ .Config.question }}'");
    for value in [None, Some(json!("1"))] {
        let server = serve().await;
        let mut s = settings();
        if let Some(v) = value {
            s.insert("question".into(), v);
        }
        let session =
            CookieSession::new_form(plan(&yaml).unwrap(), &server.base, &s, 0, 3).unwrap();
        session.test().await.unwrap();
        let site = server.site.lock().unwrap();
        let post = site.hits.iter().find(|h| h.method == Method::POST).unwrap();
        assert!(
            url::form_urlencoded::parse(post.body.as_bytes()).any(|(k, v)| k == "question"
                && v == s.get("question").and_then(Value::as_str).unwrap_or("0"))
        );
    }
    for value in [json!("2"), json!(1), Value::Null, json!([])] {
        let mut s = settings();
        s.insert("question".into(), value);
        assert!(
            CookieSession::new_form(plan(&yaml).unwrap(), "https://example.test/", &s, 0, 3)
                .is_err()
        );
    }
    for bad in [
        yaml.replace("default: '0'", "default: '2'"),
        yaml.replace("options: {'0': None, '1': First}", "options: {}"),
    ] {
        assert!(plan(&bad).is_err());
    }
}
struct Server {
    base: String,
    site: Arc<Mutex<Site>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve() -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let site = Arc::new(Mutex::new(Site::default()));
    let app = Router::new()
        .fallback(any(respond))
        .with_state(site.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server { base, site, task }
}
fn hidden(error: anyhow::Error) -> String {
    for text in [error.to_string(), format!("{error:?}")] {
        for secret in [
            USER,
            PASS,
            "fresh & token",
            "never expose",
            "untrusted-secret",
        ] {
            assert!(!text.contains(secret), "error leaked fixture value");
        }
    }
    error.to_string()
}

#[test]
fn compiler_bounds_and_credential_types_fail_before_network() {
    assert!(plan(YAML).is_ok());
    for slot in ["form#login", "'.login-error'", "'a[href=\"logout\"]'"] {
        for selector in [r#":root:contains("{")"#, r#":root:contains("}")"#] {
            assert!(plan(&YAML.replace(slot, &format!("'{selector}'"))).is_ok());
        }
        for selector in [r#":root:contains("{{ .Config.password }}")"#, "["] {
            assert!(plan(&YAML.replace(slot, &format!("'{selector}'"))).is_err());
        }
    }
    assert!(plan(&YAML.replace("path: login", "path: 'login{literal}'")).is_err());
    // Complete pinned provider plans also pass the form compiler; this does not admit them.
    for id in [
        "4thd",
        "empornium",
        "hdtorrents",
        "pixelcove",
        "diablotorrent",
    ] {
        assert!(
            plan(&std::fs::read_to_string(format!("vendor/jackett/definitions/{id}.yml")).unwrap())
                .is_ok(),
            "{id}"
        );
    }
    for bad in [
        YAML.replace("UTF-8", "ISO-8859-15"),
        YAML.replace("UTF-8", "[UTF-8]"),
        YAML.replace("method: form", "method: cookie"),
        YAML.replace("method: form", "method: [form]"),
        YAML.replace("method: form", "method: form\n  selectors: true"),
        YAML.replace("method: form", "method: form\n  selectorinputs: {}"),
        YAML.replace("method: form", "method: form\n  headers: {}"),
        YAML.replace("form: form#login", "form: '['"),
        YAML.replace("form: form#login", "form: '{{ .Config.username }}'"),
        YAML.replace("path: account, selector:", "path: account, unexpected:"),
        YAML.replace(".Config.username", ".Config.undeclared"),
        YAML.replace(".Config.username", ".Config.username | trim"),
        YAML.replace("type: text", "type: checkbox"),
        YAML.replace(
            "paths: [{path: search}]",
            "paths: [{path: search, method: put}]",
        ),
        YAML.replace(
            "paths: [{path: search}]",
            "paths: [{path: search, method: [get]}]",
        ),
        YAML.replace(
            "paths: [{path: search}]",
            "paths: [{path: search, response: {type: csv}}]",
        ),
        YAML.replace(
            "paths: [{path: search}]",
            "paths: [{path: search, headers: {X-Test: abc}}]",
        ),
        YAML.replace("search:\n", "search:\n  followredirect: true\n"),
        YAML.replace("path: login", "path: login\n  path: other"),
        format!("{YAML}\ndownload: {{path: details}}\n"),
        format!("{YAML}\ncertificates: []\n"),
    ] {
        assert!(plan(&bad).is_err(), "unsupported form schema accepted");
    }
    for value in [
        Value::Null,
        json!(false),
        json!({}),
        json!(4),
        json!(""),
        json!("x".repeat(8193)),
    ] {
        let mut s = settings();
        s.insert("password".into(), value);
        assert!(
            CookieSession::new_form(plan(YAML).unwrap(), "https://example.test/", &s, 0, 3)
                .is_err()
        );
    }
    for path in [
        "https://other.test/login",
        "//other.test/login",
        "login#fragment",
    ] {
        assert!(CookieSession::new_form(
            plan(&YAML.replace("path: login", &format!("path: '{path}'"))).unwrap(),
            "https://example.test/",
            &settings(),
            0,
            3
        )
        .is_err());
    }
}

#[tokio::test]
async fn form_controls_csrf_overrides_encoding_reuse_and_download() {
    let server = serve().await;
    let session = session(&server.base, YAML);
    assert_eq!(session.fetch_html("search").await.unwrap(), LOGGED);
    assert_eq!(session.fetch_html("search").await.unwrap(), LOGGED);
    assert_eq!(
        session.fetch_download("download").await.unwrap(),
        b"d4:infod4:name4:testee"
    );
    let s = server.site.lock().unwrap();
    assert_eq!(s.posts, 1);
    // The validated session sends no test before its second search; downloads
    // still check it (docs/session-validation.md).
    assert_eq!(
        s.hits.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(),
        [
            "/login",
            "/submit",
            "/welcome",
            "/account",
            "/search",
            "/search",
            "/account",
            "/download"
        ]
    );
    let post = &s.hits[1];
    let fields: Vec<_> = url::form_urlencoded::parse(post.body.as_bytes())
        .into_owned()
        .collect();
    assert_eq!(
        fields,
        [
            ("csrf", "fresh & token"),
            ("username", USER),
            ("dup", "last"),
            ("checked", "yes"),
            ("empty", ""),
            ("legend", "yes"),
            ("radio", "last"),
            ("keep", "yes"),
            ("password", PASS)
        ]
        .map(|(k, v)| (k.to_string(), v.to_string()))
    );
    assert!(post.body.contains("username=user+%26+caf%C3%A9"));
    assert!(post.body.contains("password=password%2Bsecret%3Dfixture"));
    assert_eq!(post.headers["cookie"], "csrf=landing");
    assert_eq!(post.headers["referer"], format!("{}login", server.base));
    assert_eq!(
        post.headers["content-type"],
        "application/x-www-form-urlencoded"
    );
    assert_eq!(s.hits[2].method, Method::GET);
    assert!(s.hits[2].body.is_empty());
    for hit in s.hits.iter().filter(|h| h.method == Method::GET) {
        assert!(hit.body.is_empty());
        assert!(!hit.path.contains(PASS));
    }
}

#[tokio::test]
async fn expired_marker_and_401_allow_only_one_search_relogin() {
    for unauthorized in [false, true] {
        let server = serve().await;
        let session = session(&server.base, YAML);
        session.test().await.unwrap();
        {
            let mut s = server.site.lock().unwrap();
            s.search_failures = 1;
            s.search_unauthorized = unauthorized;
        }
        assert_eq!(session.fetch_html("search").await.unwrap(), LOGGED);
        assert_eq!(server.site.lock().unwrap().posts, 2);
        {
            let mut s = server.site.lock().unwrap();
            s.search_failures = 2;
        }
        assert!(hidden(session.fetch_html("search").await.unwrap_err())
            .contains("expected login selector missing"));
        assert_eq!(server.site.lock().unwrap().posts, 3);
        // A subsequent request can recover; failed authentication never poisons the session lock.
        assert_eq!(session.fetch_html("search").await.unwrap(), LOGGED);
        assert_eq!(server.site.lock().unwrap().posts, 4);
    }
}

#[tokio::test]
async fn expired_preflight_reauthenticates_but_http_errors_do_not_loop() {
    let server = serve().await;
    let session = session(&server.base, YAML);
    session.test().await.unwrap();
    server.site.lock().unwrap().logged = false;
    session.test().await.unwrap();
    assert_eq!(server.site.lock().unwrap().posts, 2);
    // An explicit 401 is an expiry signal, but a 503 cannot trigger credential retries.
    server.site.lock().unwrap().account_status = StatusCode::UNAUTHORIZED;
    assert!(session.test().await.is_err()); // Reauth occurred, final positive test still denied.
    assert_eq!(server.site.lock().unwrap().posts, 3);
    server.site.lock().unwrap().account_status = StatusCode::SERVICE_UNAVAILABLE;
    assert!(hidden(session.test().await.unwrap_err()).contains("HTTP error"));
    assert_eq!(server.site.lock().unwrap().posts, 3);
}

#[tokio::test]
async fn positive_login_required_and_post_error_redacted() {
    for error_page in [true, false] {
        let server = serve().await;
        {
            let mut s = server.site.lock().unwrap();
            if error_page {
                s.post_body =
                    "<div class=login-error>password+secret=fixture untrusted-secret</div>".into();
            } else {
                s.account_body = Some("<form>Login</form>".into());
            }
        }
        let session = session(&server.base, YAML);
        let error = hidden(session.fetch_html("search").await.unwrap_err());
        assert!(error.contains(if error_page {
            "rejected form login"
        } else {
            "expected login selector missing"
        }));
        let s = server.site.lock().unwrap();
        assert_eq!(s.posts, 1);
        assert!(!s.hits.iter().any(|h| h.path == "/search"));
    }
}

#[tokio::test]
async fn captcha_challenge_malformed_or_inert_forms_never_submit_credentials() {
    let captcha_yaml = YAML.replace(
        "method: form",
        "method: form\n  captcha: {type: image, selector: img.captcha, input: code}",
    );
    for landing in [
        "<p>Missing form</p>".into(),
        format!("<template>{FORM}</template>"),
        format!("{FORM}<img class=captcha>"),
        format!("{FORM}<script src=simpleCaptcha.js></script>"),
        FORM.replace("id=login", "id=login enctype=multipart/form-data"),
        FORM.replace("name=csrf", "name=csrf form=other"),
        format!("{FORM}<input form=login type=radio name=radio checked>"),
        FORM.replace("fresh &amp; token", &"x".repeat(8193)),
        FORM.replace(
            "</form>",
            &format!("{}</form>", "<input name=x>".repeat(129)),
        ),
    ] {
        let server = serve().await;
        server.site.lock().unwrap().landing = landing;
        assert!(session(&server.base, &captcha_yaml).test().await.is_err());
        assert_eq!(server.site.lock().unwrap().posts, 0);
    }
    // Declared CAPTCHA absent: the normal login path remains usable.
    let server = serve().await;
    session(&server.base, &captcha_yaml).test().await.unwrap();
    assert_eq!(server.site.lock().unwrap().posts, 1);
    for refresh in [false, true] {
        let server = serve().await;
        {
            let mut s = server.site.lock().unwrap();
            if refresh {
                s.refresh = true;
            } else {
                s.landing_challenge = true;
            }
        }
        assert!(session(&server.base, YAML).test().await.is_err());
        assert_eq!(server.site.lock().unwrap().posts, 0);
    }
    let server = serve().await;
    let session = session(&server.base, YAML);
    session.test().await.unwrap();
    server.site.lock().unwrap().search_challenge = true;
    assert!(hidden(session.fetch_html("search").await.unwrap_err()).contains("Cloudflare"));
    assert_eq!(server.site.lock().unwrap().posts, 1);
}

#[tokio::test]
async fn actions_redirects_are_origin_scoped_and_post_replay_bounded() {
    let foreign = serve().await;
    for action in [
        format!("{}untrusted-secret", foreign.base),
        "//other.invalid/untrusted-secret".into(),
        "https://user:password@other.invalid/".into(),
        "submit#fragment".into(),
    ] {
        let server = serve().await;
        server.site.lock().unwrap().landing =
            FORM.replace("action=submit", &format!("action='{action}'"));
        hidden(session(&server.base, YAML).test().await.unwrap_err());
        assert_eq!(server.site.lock().unwrap().posts, 0);
    }
    for status in [StatusCode::SEE_OTHER, StatusCode::TEMPORARY_REDIRECT] {
        let server = serve().await;
        {
            let mut s = server.site.lock().unwrap();
            s.post_status = status;
            s.post_location = foreign.base.clone();
        }
        hidden(session(&server.base, YAML).test().await.unwrap_err());
        assert_eq!(server.site.lock().unwrap().posts, 1);
    }
    assert!(foreign.site.lock().unwrap().hits.is_empty());
    // 307 preserves method and form, but the redirect chain is capped at five hops.
    let server = serve().await;
    {
        let mut s = server.site.lock().unwrap();
        s.post_status = StatusCode::TEMPORARY_REDIRECT;
        s.post_location = "/submit-again".into();
    }
    assert!(
        hidden(session(&server.base, YAML).test().await.unwrap_err()).contains("redirect rejected")
    );
    let s = server.site.lock().unwrap();
    assert_eq!(s.posts, 6);
    let posts: Vec<_> = s.hits.iter().filter(|h| h.method == Method::POST).collect();
    assert!(posts.iter().all(|h| h.body == posts[0].body));
}

#[tokio::test]
async fn concurrent_calls_share_one_login_and_cancellation_releases_writer() {
    let server = serve().await;
    let session = Arc::new(session(&server.base, YAML));
    let calls = (0..8).map(|_| {
        let s = session.clone();
        tokio::spawn(async move { s.fetch_html("search").await })
    });
    for result in futures_util::future::join_all(calls).await {
        assert_eq!(result.unwrap().unwrap(), LOGGED);
    }
    assert_eq!(server.site.lock().unwrap().posts, 1);
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    {
        let mut s = server.site.lock().unwrap();
        s.logged = false;
        s.entered = Some(entered.clone());
        s.release = Some(release.clone());
    }
    let task = tokio::spawn({
        let session = session.clone();
        async move { session.test().await }
    });
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    {
        let mut s = server.site.lock().unwrap();
        s.entered = None;
        s.release = None;
        s.logged = false;
    }
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(3), session.test())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(server.site.lock().unwrap().posts, 3);
}

fn credential_post_yaml() -> String {
    YAML.replace("method: form", "method: post")
        .replace("path: login", "path: submit")
        .replace("  form: form#login\n", "")
}

#[test]
fn credential_post_accepts_only_declared_bounded_password_posts() {
    let yaml = credential_post_yaml();
    assert!(plan(&yaml).is_ok());
    // Pinned TestLogin: the test, and its selector, are optional (`CardigannIndexer.cs:896`, `:935`).
    for test in ["test: {path: account}\n  ", ""] {
        let optional = yaml.replace(
            "test: {path: account, selector: 'a[href=\"logout\"]'}\n  ",
            test,
        );
        assert_ne!(optional, yaml);
        assert!(plan(&optional).is_ok());
    }
    for extra in [
        "form: form",
        "submitpath: other",
        "captcha: {}",
        "cookies: [x=y]",
        "headers: {}",
        "selectorinputs: {}",
    ] {
        assert!(plan(&yaml.replace("method: post", &format!("method: post\n  {extra}"))).is_err());
    }
    for bad in [
        yaml.replace(".Config.password", ".Config.username"),
        yaml.replace(".Config.username", ".Config.password"),
        yaml.replace(
            "test: {path: account, selector: 'a[href=\"logout\"]'}",
            "test: {selector: 'a[href=\"logout\"]'}",
        ),
        yaml.replace("path: submit", "path: '{{ .Config.password }}'"),
        yaml.replace("path: submit", "path: submit\n  path: other"),
    ] {
        assert!(plan(&bad).is_err());
    }
    for target in [
        "https://foreign.invalid/submit",
        "//foreign.invalid/submit",
        "submit#fragment",
    ] {
        assert!(CookieSession::new_form(
            plan(&yaml.replace("path: submit", &format!("path: '{target}'"))).unwrap(),
            "https://example.test/",
            &settings(),
            0,
            3
        )
        .is_err());
    }
    for key in ["username", "password"] {
        let mut s = settings();
        s.insert(key.into(), json!(""));
        assert!(
            CookieSession::new_form(plan(&yaml).unwrap(), "https://example.test/", &s, 0, 3)
                .is_err()
        );
    }
}

#[test]
fn legacy_single_search_path_has_the_same_bounded_html_get_gate() {
    let original = credential_post_yaml();
    let single = original.replace("paths: [{path: search}]", "path: search");
    assert!(plan(&single).is_ok());
    // Form POST searches and JSON/XML responses are executable (session-search-shapes.md).
    for replacement in [
        "path: search\n  method: post",
        "path: search\n  response: {type: json}",
        "path: search\n  response: {type: xml}",
    ] {
        assert!(
            plan(&original.replace("paths: [{path: search}]", replacement)).is_ok(),
            "{replacement}"
        );
    }
    for replacement in [
        "path: search\n  paths: [{path: search}]",
        "path: search\n  method: put",
        "path: search\n  method: [get]",
        "path: search\n  response: {type: csv}",
        "path: search\n  headers: {Authorization: secret}",
        "path: search\n  followredirect: true",
        "path: [search]",
        "path: ''",
        "paths: {}",
        "paths: []",
    ] {
        assert!(
            plan(&original.replace("paths: [{path: search}]", replacement)).is_err(),
            "{replacement}"
        );
    }
}

#[tokio::test]
async fn credential_post_sends_no_form_or_stale_cookie_and_reauthenticates_once() {
    let server = serve().await;
    let session = session(&server.base, &credential_post_yaml());
    session.test().await.unwrap();
    {
        let s = server.site.lock().unwrap();
        assert_eq!(
            s.hits
                .iter()
                .map(|h| (h.method.as_str(), h.path.as_str()))
                .collect::<Vec<_>>(),
            [
                ("POST", "/submit"),
                ("GET", "/welcome"),
                ("GET", "/account")
            ]
        );
        let first = &s.hits[0];
        assert!(!first.headers.contains_key("cookie"));
        assert_eq!(first.headers["referer"], server.base);
        assert_eq!(
            first.headers["content-type"],
            "application/x-www-form-urlencoded"
        );
        let fields: BTreeMap<_, _> = url::form_urlencoded::parse(first.body.as_bytes())
            .into_owned()
            .collect();
        assert_eq!(
            fields,
            BTreeMap::from([
                ("username".into(), USER.into()),
                ("password".into(), PASS.into()),
                ("keep".into(), "yes".into())
            ])
        );
    }
    {
        let mut s = server.site.lock().unwrap();
        s.search_failures = 1;
        s.search_redirect = Some("/login?expired=1".into());
    }
    session.fetch_html("search").await.unwrap();
    let s = server.site.lock().unwrap();
    assert_eq!(s.posts, 2);
    assert!(s.hits.iter().all(|h| h.path != "/login"));
    for post in s.hits.iter().filter(|h| h.method == Method::POST) {
        assert!(
            !post.headers.contains_key("cookie"),
            "stale session reached credential POST"
        );
        assert_eq!(post.headers["referer"], server.base);
    }
}

#[tokio::test]
async fn credential_post_requires_positive_login_and_rejects_errors_and_foreign_redirects() {
    for mode in 0..4 {
        let server = serve().await;
        {
            let mut s = server.site.lock().unwrap();
            match mode {
                0 => s.account_body = Some("<p>No authenticated marker</p>".into()),
                1 => s.post_body = "<div class=login-error>untrusted-secret</div>".into(),
                2 => s.post_location = "http://127.0.0.1:1/untrusted-secret".into(),
                _ => s.post_status = StatusCode::SERVICE_UNAVAILABLE,
            }
        }
        let session = session(&server.base, &credential_post_yaml());
        hidden(session.fetch_html("search").await.unwrap_err());
        let s = server.site.lock().unwrap();
        assert_eq!(s.posts, 1);
        assert!(s
            .hits
            .iter()
            .all(|h| h.path != "/search" && h.path != "/login"));
    }
}
