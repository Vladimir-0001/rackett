//! Isolated process because solver configuration is cached for its lifetime.
#[tokio::test]
async fn invalid_source_family_fails_startup_without_echoing_value() {
    std::env::remove_var("RACKETT_FLARESOLVERR_URL");
    std::env::set_var(
        "RACKETT_FLARESOLVERR_SOURCE_IP_FAMILY",
        "private-secret-value",
    );
    let dir = tempfile::tempdir().unwrap();
    let error = rackett::engine::Engine::new(dir.path().join("config.json"))
        .await
        .err()
        .unwrap()
        .to_string();
    assert_eq!(
        error,
        "Invalid RACKETT_FLARESOLVERR_SOURCE_IP_FAMILY; expected auto, ipv4 or ipv6"
    );
}
