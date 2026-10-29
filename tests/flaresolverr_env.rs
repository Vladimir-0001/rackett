//! `RACKETT_FLARESOLVERR_URL` is read once per process, so this binary holds one test.
use rackett::engine::Engine;

#[tokio::test]
async fn invalid_solver_environment_fails_startup_without_echoing_it() {
    std::env::set_var(
        "RACKETT_FLARESOLVERR_URL",
        "http://user:secret-pass@flaresolverr:8191/",
    );
    let dir = tempfile::tempdir().unwrap();
    let error = Engine::new(dir.path().join("config.json"))
        .await
        .err()
        .unwrap()
        .to_string();
    assert!(
        error.contains("Invalid RACKETT_FLARESOLVERR_URL"),
        "{error}"
    );
    assert!(!error.contains("secret-pass"), "{error}");
}
