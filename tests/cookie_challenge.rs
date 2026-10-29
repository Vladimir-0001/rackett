//! Explicit Cloudflare challenge handling for the inactive cookie foundation.
use axum::{http::StatusCode, response::IntoResponse, routing::get, Router};
use rackett::{
    builtin_runtime::auth::{CookiePlan, CookieSession},
    cardigann,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

const SOURCE: &str = include_str!("../vendor/jackett/definitions/musopia.yml");
const AUTHENTICATED: &str =
    "<html><a href=\"logout.php\">Logout</a><table class=\"torrents\"><tbody></tbody></table></html>";

fn session(url: &str) -> CookieSession {
    let plan = CookiePlan::compile(
        &cardigann::parse(SOURCE, cardigann::Limits::default()).expect("pinned definition parses"),
    )
    .expect("pinned cookie plan compiles");
    CookieSession::new(plan, url, "session=synthetic-secret", 0, 2).unwrap()
}

async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    (
        url,
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() }),
    )
}

#[tokio::test]
async fn explicit_challenges_fail_before_search_or_download_and_are_redacted() {
    for status in [StatusCode::OK, StatusCode::FORBIDDEN] {
        // Reject challenges during preflight and after a successful preflight.
        for challenge_request in [1, 2] {
            for download in [false, true] {
                let calls = Arc::new(AtomicUsize::new(0));
                let seen = calls.clone();
                let (url, server) = serve(Router::new().fallback(get(move || {
                    let seen = seen.clone();
                    async move {
                        let request = seen.fetch_add(1, Ordering::SeqCst) + 1;
                        if request < challenge_request {
                            return (
                                StatusCode::OK,
                                [("content-type", "text/html")],
                                AUTHENTICATED,
                            )
                                .into_response();
                        }
                        let mut response = (
                            status,
                            [("cf-mitigated", "challenge"), ("content-type", "text/html")],
                            AUTHENTICATED,
                        )
                            .into_response();
                        response.headers_mut().insert(
                            "x-private-marker",
                            "arbitrary-secret-header-value".parse().unwrap(),
                        );
                        response
                    }
                })))
                .await;
                let session = session(&url);
                let error = if download {
                    session
                        .fetch_download("download.php?id=42")
                        .await
                        .unwrap_err()
                } else {
                    session.fetch_html("torrents.php").await.unwrap_err()
                }
                .to_string();
                assert!(error.contains("Cloudflare challenge detected"));
                for private in [
                    "synthetic-secret",
                    "arbitrary-secret-header-value",
                    AUTHENTICATED,
                    url.as_str(),
                ] {
                    assert!(!error.contains(private));
                }
                assert_eq!(
                    calls.load(Ordering::SeqCst),
                    challenge_request,
                    "no request after challenge and no retry"
                );
                server.abort();
            }
        }
    }
}

#[tokio::test]
async fn cloudflare_server_header_alone_does_not_reject_authenticated_html() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let (url, server) = serve(Router::new().fallback(get(move || {
        let seen = seen.clone();
        async move {
            seen.fetch_add(1, Ordering::SeqCst);
            let mut response = (
                StatusCode::OK,
                [("content-type", "text/html")],
                AUTHENTICATED,
            )
                .into_response();
            response
                .headers_mut()
                .insert("server", "cloudflare".parse().unwrap());
            response
        }
    })))
    .await;

    assert_eq!(
        session(&url).fetch_html("torrents.php").await.unwrap(),
        AUTHENTICATED
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2, "validation and one search");
    server.abort();
}
