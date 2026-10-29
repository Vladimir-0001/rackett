//! Loopback-only fixtures derived from pinned Musopia login.test (not captured site traffic).
use axum::{
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use rackett::{
    builtin,
    builtin_runtime::auth::{CookiePlan, CookieSession},
    cardigann,
    models::Indexer,
};
use serde_json::json;
use std::sync::{Arc, Mutex};
const SOURCE: &str = include_str!("../vendor/jackett/definitions/musopia.yml");
const AUTHENTICATED: &str = "<html><a href=\"logout.php\">Logout</a><table class=\"torrents\"><tbody></tbody></table></html>";
fn plan(yaml: &str) -> anyhow::Result<CookiePlan> {
    CookiePlan::compile(
        &cardigann::parse(yaml, cardigann::Limits::default())
            .map_err(|_| anyhow::anyhow!("Invalid fixture"))?,
    )
}
fn session(url: &str, cookie: &str) -> CookieSession {
    CookieSession::new(plan(SOURCE).unwrap(), url, cookie, 0, 2).unwrap()
}
async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    (
        format!("http://{}/", listener.local_addr().unwrap()),
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() }),
    )
}
#[test]
fn pinned_complete_schema_and_only_reviewed_cookie_preset_is_admitted() {
    assert!(plan(SOURCE).is_ok());
    for yaml in [
        SOURCE.replace("encoding: UTF-8", "encoding: [windows-1252]"),
        SOURCE.replace("encoding: UTF-8", "encoding: {name: windows-1252}"),
        SOURCE.replace("encoding: UTF-8", "encoding: ISO-8859-1"),
        SOURCE.replace("encoding: UTF-8", "encoding: iso-8859-1"),
        SOURCE.replace("method: cookie", "method: form"),
        SOURCE.replace("method: cookie", "method: post"),
        SOURCE.replace("method: cookie", "method: get"),
        SOURCE.replace("method: cookie", "method: cookie\n  headers: {X-Test: abc}"),
        SOURCE.replace("method: cookie", "method: cookie\n  error: []"),
        SOURCE.replace("method: cookie", "method: cookie\n  captcha: {}"),
        SOURCE.replace("method: cookie", "method: cookie\n  method: cookie"),
        SOURCE.replace("path: index.php", "path: '{{ .Config.cookie }}'"),
        SOURCE.replace("selector: a[href=\"logout.php\"]", "selector: '['"),
        SOURCE.replace("search:\n", "search:\n  headers: {User-Agent: [test]}\n"),
        SOURCE.replace("{{ .Config.cookie }}", "{{ .Config.password }}"),
        format!("{SOURCE}\nfollowredirect: true\n"),
        format!("{SOURCE}\nlogin: {{method: form}}\n"),
        format!("{SOURCE}\nsearch: {{headers: {{X-Test: abc}}}}\n"),
        format!("{SOURCE}\nencoding: windows-1252\n"),
        format!("{SOURCE}\ndownload: {{headers: {{X-Test: abc}}}}\n"),
        SOURCE.replace("  - name: cookie", "  - name: cookie\n    name: password"),
        SOURCE.replace("    type: text", "    type: text\n    type: info"),
    ] {
        assert!(plan(&yaml).is_err(), "Unexpected schema admission");
    }
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
        assert!(builtin::READY_IDS.contains(&id));
    }
    // The NexusPHP family test pins the exact union of admitted IDs.
    let row = builtin::catalog()
        .iter()
        .find(|r| r["id"] == "musopia")
        .unwrap();
    assert_eq!(row["ready"], true);
    assert!(row["blocked_reason"].is_null());
}
#[test]
fn cookie_setting_redacted_and_preserved_without_exposing_secret() {
    let old = Indexer {
        kind: "cardigann".into(),
        url: "http://example.test/".into(),
        definition: Some(
            json!({"builtin_id":"musopia","settings":{"cookie":"session=synthetic-secret","freeleech":true}}),
        ),
        ..Default::default()
    };
    let public = builtin::public_definition(&old).unwrap();
    assert!(public["settings"].get("cookie").is_none());
    assert!(!public.to_string().contains("synthetic-secret"));
    let mut edited = Indexer {
        name: "Edited".into(),
        definition: Some(public),
        ..old.clone()
    };
    builtin::preserve_secrets(&mut edited, &old);
    assert_eq!(edited.definition, old.definition);
    edited.definition.as_mut().unwrap()["settings"]["cookie"] = json!("session=replacement");
    builtin::preserve_secrets(&mut edited, &old);
    assert_eq!(
        edited.definition.as_ref().unwrap()["settings"]["cookie"],
        "session=replacement"
    );
    for cookie in [
        "",
        "bad",
        "session=",
        "session=__jackett_expired__",
        "session=a; session=b",
        "session=secret\r\nHost: evil",
        "session=a b",
        "session=a\t",
        "\tsession=a",
        "session=a\u{a0}",
        "=secret",
        "a=1;",
        "session=\"a\"",
    ] {
        let error = CookieSession::new(plan(SOURCE).unwrap(), "http://example.test/", cookie, 0, 2)
            .err()
            .unwrap()
            .to_string();
        assert_eq!(error, "Invalid cookie setting");
    }
    assert!(CookieSession::new(
        plan(&SOURCE.replace("path: index.php", "path: https://other.test/")).unwrap(),
        "http://example.test/",
        "a=b",
        0,
        2
    )
    .is_err());
}
#[tokio::test]
async fn authenticated_test_empty_search_download_and_concurrent_source_isolation() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = seen.clone();
    let (url, server) = serve(Router::new().fallback(get(
        move |uri: axum::http::Uri, headers: HeaderMap| {
            let captured = captured.clone();
            async move {
                captured.lock().unwrap().push((
                    uri.path().to_owned(),
                    headers["cookie"].to_str().unwrap().to_owned(),
                ));
                if uri.path() == "/download.php" {
                    (
                        [("content-type", "application/x-bittorrent")],
                        "d4:infod4:name4:testee",
                    )
                        .into_response()
                } else {
                    (
                        [("content-type", "text/html; charset=UTF-8")],
                        AUTHENTICATED,
                    )
                        .into_response()
                }
            }
        },
    )))
    .await;
    let a = session(&url, "session=source-a");
    let b = session(&url, "session=source-b");
    let (a_result, b_result) =
        tokio::join!(a.fetch_html("torrents.php"), b.fetch_html("torrents.php"));
    assert_eq!(a_result.unwrap(), AUTHENTICATED);
    assert_eq!(b_result.unwrap(), AUTHENTICATED);
    a.test().await.unwrap();
    assert_eq!(
        a.fetch_download("download.php?id=42").await.unwrap(),
        b"d4:infod4:name4:testee"
    );
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 7);
    assert_eq!(
        seen.iter().filter(|(_, c)| c == "session=source-b").count(),
        2
    );
    assert_eq!(
        seen.iter()
            .filter(|(p, c)| p == "/download.php" && c == "session=source-a")
            .count(),
        1
    );
    server.abort();
}
#[tokio::test]
async fn missing_expired_challenge_error_and_invalid_cookie_updates_fail_without_retries_or_leaks()
{
    for (status, body, ct, set_cookie) in [
        (StatusCode::OK, "", "text/html", false),
        (
            StatusCode::OK,
            "<form>expired synthetic-secret</form>",
            "text/html",
            false,
        ),
        (
            StatusCode::OK,
            "<h1>Challenge synthetic-secret</h1>",
            "text/html",
            false,
        ),
        (
            StatusCode::FORBIDDEN,
            "synthetic-secret",
            "text/html",
            false,
        ),
        (StatusCode::OK, AUTHENTICATED, "application/json", false),
        (
            StatusCode::OK,
            AUTHENTICATED,
            "text/html; charset=iso-8859-1",
            false,
        ),
        (StatusCode::OK, AUTHENTICATED, "text/html", true),
    ] {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = calls.clone();
        let (url, server) = serve(Router::new().fallback(get(move || {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move {
                let mut response = (status, [("content-type", ct)], body).into_response();
                if set_cookie {
                    response.headers_mut().insert(
                        "set-cookie",
                        "session=synthetic-secret\tbad".parse().unwrap(),
                    );
                }
                response
            }
        })))
        .await;
        let error = session(&url, "session=synthetic-secret")
            .fetch_html("torrents.php")
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains("synthetic-secret"));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        server.abort();
    }
    // Expiry between successful validation and search must fail once, without relogin/replay.
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c = calls.clone();
    let (url, server) = serve(
        Router::new()
            .route(
                "/index.php",
                get(|| async { axum::response::Html(AUTHENTICATED) }),
            )
            .route(
                "/torrents.php",
                get(move || {
                    c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    async { "<form>Expired</form>" }
                }),
            ),
    )
    .await;
    assert!(session(&url, "a=b")
        .fetch_html("torrents.php")
        .await
        .is_err());
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    server.abort();
}
#[tokio::test]
async fn same_origin_validation_redirect_and_cross_origin_no_send() {
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let h = hits.clone();
    let (other, other_server) = serve(Router::new().fallback(get(move || {
        h.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        async { axum::response::Html(AUTHENTICATED) }
    })))
    .await;
    let foreign = other.clone();
    let (url, server) = serve(
        Router::new()
            .route(
                "/index.php",
                get(|| async { axum::response::Redirect::temporary("/home") }),
            )
            .route(
                "/home",
                get(|| async { axum::response::Html(AUTHENTICATED) }),
            )
            .route(
                "/escape",
                get(move || {
                    let foreign = foreign.clone();
                    async move { axum::response::Redirect::temporary(&foreign) }
                }),
            ),
    )
    .await;
    let auth = session(&url, "a=b");
    auth.test().await.unwrap();
    assert!(auth.fetch_html(&other).await.is_err());
    assert!(auth.fetch_download("escape").await.is_err());
    assert!(auth.fetch_html("escape").await.is_err());
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    server.abort();
    other_server.abort();
}

#[tokio::test]
async fn response_byte_limits_deadline_and_spacing_are_enforced() {
    let (url, server) = serve(
        Router::new()
            .route(
                "/index.php",
                get(|| async { axum::response::Html(AUTHENTICATED) }),
            )
            .route(
                "/oversize",
                get(|| async {
                    (
                        [("content-type", "text/html")],
                        "x".repeat(8 * 1024 * 1024 + 1),
                    )
                }),
            )
            .route(
                "/large-download",
                get(|| async { "x".repeat(32 * 1024 * 1024 + 1) }),
            )
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    axum::response::Html(AUTHENTICATED)
                }),
            ),
    )
    .await;
    let auth = session(&url, "a=b");
    assert!(auth
        .fetch_html("oversize")
        .await
        .unwrap_err()
        .to_string()
        .contains("byte budget"));
    assert!(auth
        .fetch_download("large-download")
        .await
        .unwrap_err()
        .to_string()
        .contains("byte budget"));
    let short = CookieSession::new(plan(SOURCE).unwrap(), &url, "a=b", 0, 1).unwrap();
    assert_eq!(
        short.fetch_html("slow").await.unwrap_err().to_string(),
        "Cookie search deadline exceeded"
    );
    server.abort();
    let times = Arc::new(Mutex::new(Vec::new()));
    let captured = times.clone();
    let (url, server) = serve(Router::new().fallback(get(move || {
        captured.lock().unwrap().push(std::time::Instant::now());
        async { axum::response::Html(AUTHENTICATED) }
    })))
    .await;
    let auth = CookieSession::new(plan(SOURCE).unwrap(), &url, "a=b", 70, 2).unwrap();
    let (one, two) = tokio::join!(auth.fetch_html("torrents.php"), auth.test());
    one.unwrap();
    two.unwrap();
    let times = times.lock().unwrap();
    assert_eq!(times.len(), 3);
    assert!(times
        .windows(2)
        .all(|pair| pair[1].duration_since(pair[0]) >= std::time::Duration::from_millis(60)));
    server.abort();
}
#[tokio::test]
async fn auth_redirect_budget_and_search_redirects_are_not_replayed() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c = calls.clone();
    let (url, server) = serve(
        Router::new()
            .route(
                "/index.php",
                get(|| async { axum::response::Redirect::temporary("/second") }),
            )
            .route(
                "/second",
                get(|| async { axum::response::Redirect::temporary("/third") }),
            )
            .route(
                "/third",
                get(move || {
                    c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    async { axum::response::Html(AUTHENTICATED) }
                }),
            ),
    )
    .await;
    assert!(session(&url, "a=b")
        .test()
        .await
        .unwrap_err()
        .to_string()
        .contains("redirect rejected"));
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn authentication_uses_bounded_scripting_off_dom_and_inert_templates() {
    for (body, accepted) in [
        (
            "<noscript><a href=logout.php>Logout</a></noscript>".to_owned(),
            true,
        ),
        (
            "<template><a href=logout.php>Logout</a></template>".to_owned(),
            false,
        ),
        (
            format!("{AUTHENTICATED}<select><option>Filter</option></select>"),
            true,
        ),
        (
            format!("{AUTHENTICATED}<div><template shadowrootmode=closed></template></div>"),
            false,
        ),
        (
            format!(
                "{}{}{}",
                "<div>".repeat(130),
                AUTHENTICATED,
                "</div>".repeat(130)
            ),
            false,
        ),
    ] {
        let (url, server) = serve(Router::new().fallback(get(move || {
            let body = body.clone();
            async { axum::response::Html(body) }
        })))
        .await;
        let result = session(&url, "session=synthetic-secret").test().await;
        assert_eq!(result.is_ok(), accepted);
        if let Err(error) = result {
            assert!(!error.to_string().contains("synthetic-secret"));
        }
        server.abort();
    }
}

#[test]
fn local_login_markers_accept_filter_forms_but_never_inert_or_select_content() {
    use rackett::cardigann_exec::query_selector_exists as exists;
    let filter = "<form><select name=category><option>All categories</option></select></form>";
    for (selector, marker) in [
        ("a[href='logout.php']", "<a href=logout.php>Logout</a>"),
        (
            "a[href^='logout.php?']",
            "<a href='logout.php?key=fixture'>Logout</a>",
        ),
        (
            "a[href$='logout.php']",
            "<a href='/account/logout.php'>Logout</a>",
        ),
        (
            "a[href*='logout.php']",
            "<a href='/logout.php?key=fixture'>Logout</a>",
        ),
        ("#nav_userinfo", "<div id=nav_userinfo>User</div>"),
        ("a.username", "<a class=username>User</a>"),
        ("div#member_info_bar", "<div id=member_info_bar>User</div>"),
        (
            "a[onclick=\"document.getElementById('logout').submit();\"]",
            "<a onclick=\"document.getElementById('logout').submit();\">Logout</a>",
        ),
        (
            "a[data-marker=':has(select) > x']",
            "<a data-marker=':has(select) > x'>Logout</a>",
        ),
        ("a.absent, #logout", "<a id=logout>Logout</a>"),
    ] {
        for page in [
            format!("{filter}{marker}"),
            format!("{marker}{filter}"),
            format!("<noscript>{marker}</noscript>{filter}"),
        ] {
            assert!(exists(&page, selector).unwrap(), "{selector}");
        }
        assert!(!exists(filter, selector).unwrap(), "{selector}");
        assert!(
            !exists(&format!("<template>{marker}</template>{filter}"), selector).unwrap(),
            "{selector}"
        );
    }
    for page in [
        "<select id=logout></select>",
        "<select><option id=logout>Logout</option></select>",
        "<template><select></select><a id=logout>Logout</a></template>",
    ] {
        assert!(!exists(page, "#logout").unwrap());
    }
    assert!(exists("<option id=logout>Logout</option>", "#logout").unwrap());
    assert!(exists("<svg><select id=logout></select></svg>", "#logout").unwrap());
    // A later genuine marker must still be found after an excluded subtree.
    assert!(exists(
        "<select><option id=logout>Inert</option></select><a id=logout>Logout</a>",
        "#logout"
    )
    .unwrap());
}

#[test]
fn malformed_select_tokens_cannot_create_a_false_login_marker_after_the_select() {
    use rackett::cardigann_exec::query_selector_exists as exists;
    for page in [
        "<select><xmp></select><a id=logout>Secret</a></xmp>",
        "<select><option><xmp></select><a id=logout>Secret</a></xmp>",
        "<select><option><span id=logout>Secret</span></option></select><a id=logout>Logout</a>",
        "<select><button></select><a id=logout>Secret</a></button>",
        "<table><tr><td><select><tr><td><a id=logout>Secret</a>",
        "<select><input id=logout></select><a id=logout>Secret</a>",
    ] {
        let error = exists(page, "#logout").unwrap_err();
        assert_eq!(
            error,
            "HTML authentication document has unsupported select markup"
        );
        assert!(!error.contains("Secret"));
        assert!(!error.contains("logout"));
    }
    // An attacker cannot spoof the exact-error suffix inside attribute/text data
    // or change parsing of a genuine, ordinary marker outside a normal form.
    assert!(exists("<p title=' in insertion mode InSelect'>Unexpected token</p><select><option>All</option></select><a id=logout>Logout</a>", "#logout").unwrap());
}

#[test]
fn login_structural_selectors_and_shadow_roots_keep_the_existing_guards() {
    use rackett::cardigann_exec::query_selector_exists as exists;
    for selector in [
        "html a",
        "html > body",
        "a + a",
        "a ~ a",
        "a:not(.guest)",
        "a:empty",
        "a:first-child",
        "html:has(a)",
        ":is(a, span)",
        ":where(a)",
        "a:contains(Logout)",
        "a, a:not(.guest)",
        "a[href!=login.php]",
    ] {
        let err = exists(
            "<a>Logout</a><select><option>Filter</option></select>",
            selector,
        )
        .unwrap_err();
        assert!(err.contains("select"), "{selector}: {err}");
    }
    for page in [
        "<div><template shadowrootmode=open></template></div>",
        "<div><template shadowrootmode=closed></template></div>",
        "<div><template shadowrootmode=OPEN></template></div>",
        "<div><template shadowrootmode=bogus></template></div>",
    ] {
        for marker in [
            format!("<a id=logout>Logout</a>{page}"),
            format!("{page}<a id=logout>Logout</a>"),
        ] {
            assert!(exists(&marker, "#logout").unwrap_err().contains("shadow"));
        }
    }
    assert!(exists(&"x".repeat(8 * 1024 * 1024 + 1), "html")
        .unwrap_err()
        .contains("byte budget"));
    assert!(exists("<a id=logout></a>", &"a".repeat(4097)).is_err());
}

#[test]
fn pinned_cookie_family_uses_positive_local_login_selectors_without_admitting_it() {
    let mut checked = Vec::new();
    for (id, source) in builtin::SOURCES {
        let root = cardigann::parse(source, cardigann::Limits::default()).unwrap();
        if CookiePlan::compile(&root).is_err() {
            continue;
        }
        let Ok(plan) = rackett::cardigann_exec::Plan::compile(&root) else {
            continue;
        };
        if plan.blockers().iter().any(|b| b.path != "$.login") {
            continue;
        }
        let selector = root
            .get("login")
            .unwrap()
            .get("test")
            .unwrap()
            .string("selector")
            .unwrap();
        assert_eq!(
            rackett::cardigann_exec::query_selector_exists(
                "<select><option>Filter</option></select>",
                selector
            ),
            Ok(false),
            "{id}"
        );
        checked.push(*id);
    }
    assert_eq!(checked.len(), 101);
    for id in [
        "animebybelka",
        "hdtime",
        "huntorrent",
        "korsar",
        "majomparade",
        "matrix",
        "pterclub",
        "ptskit",
        "teamhd",
        "torrentmasters",
        "turktorrent",
    ] {
        assert!(checked.contains(&id), "{id}: configured User-Agent schema");
    }
    assert!(checked.contains(&"musopia"));
    assert!(builtin::COOKIE_IDS.iter().all(|id| checked.contains(id)));
}

#[test]
fn cookie_transport_rejects_search_shapes_it_cannot_execute() {
    for yaml in [
        SOURCE.replace("search:\n", "search:\n  method: post\n"),
        SOURCE.replace("search:\n", "search:\n  response: {type: json}\n"),
        SOURCE.replace(
            "    - path: torrents.php",
            "    - path: torrents.php\n      method: post",
        ),
        SOURCE.replace(
            "    - path: torrents.php",
            "    - path: torrents.php\n      response: {type: json}",
        ),
    ] {
        assert_eq!(
            plan(&yaml).err().unwrap().to_string(),
            "Cookie search requires plain HTML GET paths"
        );
    }
}

#[tokio::test]
async fn refreshed_cookie_flows_through_login_search_download_and_same_origin_redirects() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = seen.clone();
    let (url, server) = serve(Router::new().fallback(get(
        move |uri: axum::http::Uri, h: HeaderMap| {
            let seen = captured.clone();
            async move {
                let cookie = h
                    .get("cookie")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_owned();
                seen.lock()
                    .unwrap()
                    .push((uri.path().to_owned(), cookie.clone()));
                match uri.path() {
                    "/index.php" if cookie == "session=seed" => {
                        let mut r =
                            axum::response::Redirect::temporary("/verified").into_response();
                        r.headers_mut().append(
                            "set-cookie",
                            "session=rotated; Path=/; HttpOnly; SameSite=Lax"
                                .parse()
                                .unwrap(),
                        );
                        r
                    }
                    "/index.php" | "/verified" | "/torrents.php" if cookie == "session=rotated" => {
                        axum::response::Html(AUTHENTICATED).into_response()
                    }
                    "/download.php" if cookie == "session=rotated" => {
                        let mut r = axum::response::Redirect::temporary("/torrent").into_response();
                        r.headers_mut()
                            .append("set-cookie", "session=final; Path=/".parse().unwrap());
                        r
                    }
                    "/torrent" if cookie == "session=final" => {
                        b"d4:infod4:name4:testee".as_slice().into_response()
                    }
                    _ => (StatusCode::UNAUTHORIZED, "private-response").into_response(),
                }
            }
        },
    )))
    .await;
    let auth = session(&url, "session=seed");
    assert_eq!(
        auth.fetch_html("torrents.php").await.unwrap(),
        AUTHENTICATED
    );
    assert_eq!(
        auth.fetch_download("download.php").await.unwrap(),
        b"d4:infod4:name4:testee"
    );
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            ("/index.php".into(), "session=seed".into()),
            ("/verified".into(), "session=rotated".into()),
            ("/torrents.php".into(), "session=rotated".into()),
            ("/index.php".into(), "session=rotated".into()),
            ("/download.php".into(), "session=rotated".into()),
            ("/torrent".into(), "session=final".into()),
        ]
    );
    // A separately configured source on the same origin never inherits the first's state.
    assert!(session(&url, "session=other").test().await.is_err());
    assert_eq!(seen.lock().unwrap().last().unwrap().1, "session=other");
    server.abort();
}

#[tokio::test]
async fn deleted_cookie_never_reappears_on_the_next_request_or_next_search() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = seen.clone();
    let (url, server) = serve(Router::new().fallback(get(
        move |uri: axum::http::Uri, h: HeaderMap| {
            let seen = captured.clone();
            async move {
                let cookie = h
                    .get("cookie")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_owned();
                seen.lock()
                    .unwrap()
                    .push((uri.path().to_owned(), cookie.clone()));
                if uri.path() == "/index.php" && cookie == "session=seed" {
                    let mut r = axum::response::Html(AUTHENTICATED).into_response();
                    r.headers_mut()
                        .append("set-cookie", "session=; Path=/; Max-Age=0".parse().unwrap());
                    r
                } else {
                    "<form>Logged out</form>".into_response()
                }
            }
        },
    )))
    .await;
    let auth = session(&url, "session=seed");
    assert!(auth.fetch_html("torrents.php").await.is_err());
    assert!(auth.fetch_html("torrents.php").await.is_err());
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            ("/index.php".into(), "session=seed".into()),
            ("/torrents.php".into(), "".into()),
            ("/index.php".into(), "".into())
        ]
    );
    server.abort();
}

#[tokio::test]
async fn cookie_scope_and_malformed_update_bounds_are_enforced_over_http() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = seen.clone();
    let (url, server) = serve(Router::new().fallback(get(
        move |uri: axum::http::Uri, h: HeaderMap| {
            let seen = captured.clone();
            async move {
                let cookie = h
                    .get("cookie")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_owned();
                seen.lock().unwrap().push((uri.path().to_owned(), cookie));
                let mut r = axum::response::Html(AUTHENTICATED).into_response();
                if uri.path() == "/index.php" {
                    for v in [
                        "scoped=yes; Path=/private",
                        "foreign=no; Domain=elsewhere.invalid; Path=/",
                        "secure=no; Secure; Path=/",
                    ] {
                        r.headers_mut().append("set-cookie", v.parse().unwrap());
                    }
                }
                if uri.path() == "/oversized" {
                    r.headers_mut().append(
                        "set-cookie",
                        format!("session={}", "secret".repeat(700)).parse().unwrap(),
                    );
                }
                r
            }
        },
    )))
    .await;
    let auth = session(&url, "session=seed");
    auth.fetch_html("private/torrents").await.unwrap();
    auth.fetch_html("private-other").await.unwrap();
    let error = auth.fetch_html("oversized").await.unwrap_err().to_string();
    assert!(error.contains("session budget"));
    assert!(!error.contains("secret"));
    auth.test().await.unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.iter()
            .find(|(p, _)| p == "/private/torrents")
            .unwrap()
            .1,
        "scoped=yes; session=seed"
    );
    assert_eq!(
        seen.iter().find(|(p, _)| p == "/private-other").unwrap().1,
        "session=seed"
    );
    assert_eq!(seen.last().unwrap().1, "session=seed");
    server.abort();
}

#[tokio::test]
async fn aidoru_ajax_table_requires_login_preflight_and_its_own_response_marker() {
    use axum::{extract::State, http::Uri, response::Html};
    use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
    const SOURCE: &str = include_str!("../vendor/jackett/definitions/aidoruonline.yml");
    struct StateData {
        mode: AtomicU8,
        hits: AtomicUsize,
    }
    async fn response(State(state): State<Arc<StateData>>, uri: Uri) -> axum::response::Response {
        state.hits.fetch_add(1, Ordering::SeqCst);
        let mode = state.mode.load(Ordering::SeqCst);
        let body = if uri.path() == "/" {
            if mode == 5 {
                "<table class=ttable_headinner></table>"
            } else {
                "<a href='torrents-details.php?id=42'>Authenticated home page</a>"
            }
        } else {
            match mode {
                0 => "<table class=ttable_headinner><tbody></tbody></table>",
                1 => "<form>Sign in fixture-private-error</form>",
                2 => "<template><table class=ttable_headinner></table></template>",
                3 => "<a href='torrents-details.php?id=42'>Home page, not results</a>",
                _ => "",
            }
        };
        Html(body).into_response()
    }
    let state = Arc::new(StateData {
        mode: AtomicU8::new(0),
        hits: AtomicUsize::new(0),
    });
    let (base, server) = serve(
        Router::new()
            .fallback(get(response))
            .with_state(state.clone()),
    )
    .await;
    let session =
        CookieSession::new(plan(SOURCE).unwrap(), &base, "session=fixture-cookie", 0, 2).unwrap();
    let body = session
        .fetch_html("get_ttable.php?searchstr=fixture")
        .await
        .unwrap();
    assert!(body.contains("ttable_headinner"));
    assert_eq!(state.hits.swap(0, Ordering::SeqCst), 2);
    for mode in 1..=5 {
        state.mode.store(mode, Ordering::SeqCst);
        let error = session
            .fetch_html("get_ttable.php")
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains("fixture-private-error"));
        assert_eq!(
            state.hits.swap(0, Ordering::SeqCst),
            if mode == 5 { 1 } else { 2 }
        );
    }
    state.mode.store(0, Ordering::SeqCst);
    assert!(session.fetch_html("other.php").await.is_err());
    assert_eq!(state.hits.load(Ordering::SeqCst), 0);
    // Other cookie definitions retain their positive login-marker check on the
    // search response, even if they happen to fetch this same URL.
    let ordinary = CookieSession::new(
        plan(&SOURCE.replace("id: aidoruonline", "id: fixture")).unwrap(),
        &base,
        "session=fixture-cookie",
        0,
        2,
    )
    .unwrap();
    assert!(ordinary.fetch_html("get_ttable.php").await.is_err());
    for changed in [
        SOURCE.replace("path: get_ttable.php", "path: other.php"),
        SOURCE.replace(
            "path: get_ttable.php",
            "path: get_ttable.php\n      method: get",
        ),
        SOURCE.replace("    path: /", "    path: index.php"),
        SOURCE.replace("table.ttable_headinner tr.t-row", "table.other tr.t-row"),
        SOURCE.replace("  rows:\n", "  rows:\n    after: 1\n"),
    ] {
        assert!(plan(&changed).is_err());
    }
    server.abort();
}

const AGENT_SOURCE: &str = include_str!("../vendor/jackett/definitions/hdtime.yml");
#[test]
fn declared_cookie_user_agent_is_bounded_and_required_before_network() {
    assert!(plan(AGENT_SOURCE).is_ok());
    for invalid in [
        AGENT_SOURCE.replace("{{ .Config.useragent }}", "literal"),
        AGENT_SOURCE.replace("{{ .Config.useragent }}", "{{ .Config.cookie }}"),
        AGENT_SOURCE.replace("User-Agent:", "Referer:"),
        AGENT_SOURCE.replace("User-Agent:", "Host:"),
        AGENT_SOURCE.replace("User-Agent:", "Cookie:"),
        AGENT_SOURCE.replace("name: useragent", "name: otheragent"),
        AGENT_SOURCE.replace("type: text", "type: password"),
        AGENT_SOURCE.replace(
            "headers:\n",
            "headers:\n    Referer: [https://foreign.invalid]\n",
        ),
        AGENT_SOURCE.replace("User-Agent: [", "User-Agent: [\"extra\", "),
    ] {
        assert_ne!(invalid, AGENT_SOURCE, "mutation must exercise declaration");
        assert!(
            plan(&invalid).is_err(),
            "unmodeled header declaration accepted"
        );
    }
    for invalid in [
        None,
        Some(""),
        Some(" "),
        Some("secret\r\nHost: evil"),
        Some("x\t"),
        Some("é"),
        Some("\u{7f}"),
    ] {
        let error = plan(AGENT_SOURCE)
            .unwrap()
            .with_user_agent(invalid)
            .err()
            .unwrap()
            .to_string();
        assert!(
            error == "Invalid cookie User-Agent setting"
                || error == "This preset requires a User-Agent setting"
        );
    }
    assert!(plan(AGENT_SOURCE)
        .unwrap()
        .with_user_agent(Some(&"x".repeat(4097)))
        .is_err());
    assert!(plan(AGENT_SOURCE)
        .unwrap()
        .with_user_agent(Some(&"x".repeat(4096)))
        .is_ok());
    let error = CookieSession::new(
        plan(AGENT_SOURCE).unwrap(),
        "http://127.0.0.1:1/",
        "session=fixture",
        0,
        2,
    )
    .err()
    .unwrap()
    .to_string();
    assert_eq!(error, "This preset requires a User-Agent setting");
    let schema = builtin::catalog()
        .iter()
        .find(|r| r["id"] == "hdtime")
        .unwrap();
    let text = schema.to_string();
    assert!(text.contains("browser session"));
    assert_eq!(
        schema["settings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == "info_useragent")
            .unwrap()["type"],
        "info"
    );
}

#[tokio::test]
async fn declared_user_agent_reaches_positive_login_search_redirect_and_download_only_same_origin()
{
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = seen.clone();
    let (base, server) = serve(Router::new().fallback(get(
        move |uri: axum::http::Uri, headers: HeaderMap| {
            let captured = captured.clone();
            async move {
                assert_eq!(headers["user-agent"], "Fixture Browser/1.0");
                captured.lock().unwrap().push(uri.path().to_owned());
                if uri.path() == "/index.php" {
                    return (StatusCode::FOUND, [("location", "/validated")]).into_response();
                }
                if uri.path() == "/redirect" {
                    return (StatusCode::FOUND, [("location", "/torrents.php")]).into_response();
                }
                if uri.path() == "/foreign" {
                    return (
                        StatusCode::FOUND,
                        [("location", "http://127.0.0.1:1/private")],
                    )
                        .into_response();
                }
                if uri.path() == "/download.php" {
                    return "d4:infod4:name4:testee".into_response();
                }
                axum::response::Html(AUTHENTICATED).into_response()
            }
        },
    )))
    .await;
    let s = CookieSession::new(
        plan(AGENT_SOURCE)
            .unwrap()
            .with_user_agent(Some("Fixture Browser/1.0"))
            .unwrap(),
        &base,
        "session=fixture",
        0,
        2,
    )
    .unwrap();
    assert_eq!(s.fetch_html("torrents.php").await.unwrap(), AUTHENTICATED);
    assert_eq!(
        s.fetch_download("download.php?id=42").await.unwrap(),
        b"d4:infod4:name4:testee"
    );
    assert!(s.fetch_html("redirect").await.is_err());
    assert!(s.fetch_html("foreign").await.is_err());
    assert_eq!(
        *seen.lock().unwrap(),
        [
            "/index.php",
            "/validated",
            "/torrents.php",
            "/index.php",
            "/validated",
            "/download.php",
            "/index.php",
            "/validated",
            "/redirect",
            "/index.php",
            "/validated",
            "/foreign"
        ]
    );
    server.abort();
}
