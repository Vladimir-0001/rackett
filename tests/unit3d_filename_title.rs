//! UNIT3D `single_file_release_use_filename` title selection.
//!
//! The fixture rows are synthetic: their shape is derived from the pinned
//! vendor/jackett/definitions/0dayfiles-api.yml (`rows: data` / `attribute:
//! attributes`, fields `name`, `num_file`, `files[0].name`), not captured from a
//! live tracker.
use axum::{
    http::{HeaderMap, StatusCode},
    routing::get,
    Router,
};
use rackett::{
    adapters,
    engine::Engine,
    models::{Indexer, SearchQuery},
    AppState,
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const DUMMY_KEY: &str = "dummy-unit3d-fixture-key";

fn rows() -> Value {
    let row = |id: u32, attrs: Value| {
        let mut attrs = attrs;
        attrs["download_link"] = json!(format!("/download/{id}"));
        attrs["details_link"] = json!(format!("/torrents/{id}"));
        json!({"type":"torrent","id":id.to_string(),"attributes":attrs})
    };
    json!({"data":[
        row(1, json!({"name":"Numeric Count","num_file":1,"files":[{"name":"numeric.count.mkv","size":1}]})),
        row(2, json!({"name":"String Count","num_file":"1","files":[{"name":"string.count.mkv"}]})),
        row(3, json!({"name":"Float Count","num_file":1.0,"files":[{"name":"float.count.mkv"}]})),
        row(4, json!({"name":"Multi File","num_file":2,"files":[{"name":"multi.part1.mkv"},{"name":"multi.part2.mkv"}]})),
        row(5, json!({"name":"Missing Count","files":[{"name":"missing.count.mkv"}]})),
        row(6, json!({"name":"Padded Count","num_file":"01","files":[{"name":"padded.count.mkv"}]})),
        row(7, json!({"name":"Spaced Count","num_file":" 1","files":[{"name":"spaced.count.mkv"}]})),
        row(8, json!({"name":"Bool Count","num_file":true,"files":[{"name":"bool.count.mkv"}]})),
        row(9, json!({"name":"Null Count","num_file":null,"files":[{"name":"null.count.mkv"}]})),
        row(10, json!({"name":"Zero Count","num_file":0,"files":[{"name":"zero.count.mkv"}]})),
        row(11, json!({"name":"Files Absent","num_file":1})),
        row(12, json!({"name":"Files Empty","num_file":1,"files":[]})),
        row(13, json!({"name":"Filename Empty","num_file":1,"files":[{"name":""}]})),
        row(14, json!({"name":"Filename Blank","num_file":1,"files":[{"name":"  \t"}]})),
        row(15, json!({"name":"Filename Number","num_file":1,"files":[{"name":12345}]})),
        row(16, json!({"name":"Filename Missing","num_file":1,"files":[{"size":1}]})),
        row(17, json!({"name":"File Is String","num_file":1,"files":["bare.string.mkv"]})),
        row(18, json!({"name":"Files Object","num_file":1,"files":{"0":{"name":"object.files.mkv"}}})),
        row(19, json!({"name":"Files String","num_file":1,"files":"files.string.mkv"})),
        // `name` is a required field in the definition: a filename never rescues it.
        row(20, json!({"num_file":1,"files":[{"name":"no.release.name.mkv"}]})),
        row(21, json!({"name":null,"num_file":1,"files":[{"name":"null.release.name.mkv"}]})),
        // `files` beside the row wrapper, not inside `attributes`, is not consulted.
        json!({"id":"22","files":[{"name":"wrapper.files.mkv"}],"attributes":{"name":"Wrapper Files","num_file":1,"download_link":"/download/22"}}),
        // Supported legacy flat row: fields sit directly on the data element.
        json!({"name":"Flat Row","num_file":"1","files":[{"name":"flat.row.mkv"}],"download_link":"/download/23"}),
        json!({"name":"Flat Multi","num_file":3,"files":[{"name":"flat.multi.mkv"}],"download_link":"/download/24"}),
    ]})
}

const DEFAULT_TITLES: &[&str] = &[
    "Numeric Count",
    "String Count",
    "Float Count",
    "Multi File",
    "Missing Count",
    "Padded Count",
    "Spaced Count",
    "Bool Count",
    "Null Count",
    "Zero Count",
    "Files Absent",
    "Files Empty",
    "Filename Empty",
    "Filename Blank",
    "Filename Number",
    "Filename Missing",
    "File Is String",
    "Files Object",
    "Files String",
    "Wrapper Files",
    "Flat Row",
    "Flat Multi",
];

const ENABLED_TITLES: &[&str] = &[
    "numeric.count.mkv",
    "string.count.mkv",
    "float.count.mkv",
    "Multi File",
    "Missing Count",
    "Padded Count",
    "Spaced Count",
    "Bool Count",
    "Null Count",
    "Zero Count",
    "Files Absent",
    "Files Empty",
    "Filename Empty",
    "Filename Blank",
    "Filename Number",
    "Filename Missing",
    "File Is String",
    "Files Object",
    "Files String",
    "Wrapper Files",
    "flat.row.mkv",
    "Flat Multi",
];

struct Upstream {
    url: String,
    auth: Arc<Mutex<Vec<String>>>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Upstream {
    fn drop(&mut self) {
        self.server.abort();
    }
}
async fn upstream(body: Value) -> Upstream {
    let auth = Arc::new(Mutex::new(Vec::new()));
    let seen = auth.clone();
    let body = body.to_string();
    let app = Router::new().route(
        "/api/torrents/filter",
        get(move |headers: HeaderMap| {
            let seen = seen.clone();
            let body = body.clone();
            async move {
                seen.lock().unwrap().push(
                    headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_owned(),
                );
                ([("content-type", "application/json")], body)
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Upstream {
        url: format!("http://{addr}/"),
        auth,
        server,
    }
}

fn unit3d(url: &str, definition: Option<Value>) -> Indexer {
    Indexer {
        id: "unit3d-fixture".into(),
        name: "UNIT3D fixture".into(),
        kind: "unit3d".into(),
        url: url.into(),
        api_key: DUMMY_KEY.into(),
        request_delay_ms: 0,
        definition,
        ..Default::default()
    }
}

async fn titles(definition: Option<Value>) -> Vec<String> {
    let up = upstream(rows()).await;
    let releases = adapters::search(
        &reqwest::Client::new(),
        &unit3d(&up.url, definition),
        &SearchQuery::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        *up.auth.lock().unwrap(),
        [format!("Bearer {DUMMY_KEY}")],
        "Bearer authentication is unchanged"
    );
    releases.into_iter().map(|r| r.title).collect()
}

#[tokio::test]
async fn default_and_disabled_keep_release_name() {
    assert_eq!(titles(None).await, DEFAULT_TITLES);
    assert_eq!(titles(Some(json!({}))).await, DEFAULT_TITLES);
    assert_eq!(
        titles(Some(json!({"single_file_release_use_filename": false}))).await,
        DEFAULT_TITLES
    );
    assert_eq!(
        titles(Some(json!({"categories": {"2000": [1]}}))).await,
        DEFAULT_TITLES
    );
}

#[tokio::test]
async fn enabled_uses_filename_only_for_single_file_rows() {
    assert_eq!(
        titles(Some(json!({"single_file_release_use_filename": true}))).await,
        ENABLED_TITLES
    );
    assert_eq!(
        titles(Some(
            json!({"single_file_release_use_filename": true, "categories": {"2000": [1]}})
        ))
        .await,
        ENABLED_TITLES
    );
}

#[tokio::test]
async fn enabled_filename_keeps_other_release_fields() {
    let up = upstream(rows()).await;
    let indexer = unit3d(
        &up.url,
        Some(json!({"single_file_release_use_filename": true})),
    );
    let releases = adapters::search(&reqwest::Client::new(), &indexer, &SearchQuery::default())
        .await
        .unwrap();
    let first = &releases[0];
    assert_eq!(first.title, "numeric.count.mkv");
    assert!(first.link.ends_with("/download/1"));
    assert!(first.details.as_deref().unwrap().ends_with("/torrents/1"));
    assert_eq!(first.guid, first.details.clone().unwrap());
    assert_eq!(first.attributes["files"], "1");
    assert!(releases
        .iter()
        .all(|r| !r.link.contains(".mkv") && !r.guid.contains(".mkv")));
}

#[test]
fn setting_is_strictly_boolean_and_unknown_fields_stay_rejected() {
    let ok =
        |definition: Value| adapters::validate(&unit3d("http://127.0.0.1:9/", Some(definition)));
    assert!(ok(json!({"single_file_release_use_filename": true})).is_ok());
    assert!(ok(json!({"single_file_release_use_filename": false})).is_ok());
    for bad in [
        json!("true"),
        json!("false"),
        json!(1),
        json!(0),
        json!(null),
        json!("yes"),
        json!([true]),
        json!({"value": true}),
    ] {
        let err = ok(json!({"single_file_release_use_filename": bad})).unwrap_err();
        assert!(err.to_string().contains("must be a boolean"), "{err}");
    }
    for unknown in [
        json!({"single_file_release_use_file_name": true}),
        json!({"title": "/name"}),
        json!({"settings": {"single_file_release_use_filename": true}}),
    ] {
        assert!(ok(unknown)
            .unwrap_err()
            .to_string()
            .contains("Unknown adapter definition field"));
    }
    // The option belongs to UNIT3D only.
    let mut rss = unit3d(
        "http://127.0.0.1:9/",
        Some(json!({"single_file_release_use_filename": true})),
    );
    rss.kind = "rss".into();
    assert!(adapters::validate(&rss).is_err());
}

async fn call(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    use tower::ServiceExt;
    let request = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1:9117")
        .header("origin", "http://127.0.0.1:9117")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
fn router(engine: Arc<Engine>) -> Router {
    rackett::api::router().with_state(AppState { engine })
}
fn result_titles(response: &Value) -> Vec<&str> {
    assert_eq!(response["errors"], json!([]), "{response}");
    response["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["title"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn configured_option_survives_create_restart_and_search() {
    let up = upstream(rows()).await;
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    let engine = Arc::new(Engine::new(config.clone()).await.unwrap());
    let app = router(engine.clone());

    let (status, _) = call(
        &app,
        "POST",
        "/api/indexers",
        json!({"name":"UNIT3D fixture","kind":"unit3d","url":up.url,"api_key":DUMMY_KEY,
               "request_delay_ms":0,"definition":{"single_file_release_use_filename":"true"}}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "string flag is not coerced"
    );

    let (status, created) = call(
        &app,
        "POST",
        "/api/indexers",
        json!({"name":"UNIT3D fixture","kind":"unit3d","url":up.url,"api_key":DUMMY_KEY,
               "request_delay_ms":0,"definition":{"single_file_release_use_filename":true}}),
    )
    .await;
    assert!(status.is_success(), "{created}");
    let id = created["id"].as_str().unwrap().to_owned();

    let (status, results) = call(
        &app,
        "GET",
        &format!("/api/search?indexers={id}&q=first"),
        Value::Null,
    )
    .await;
    assert!(status.is_success());
    assert_eq!(result_titles(&results), ENABLED_TITLES);
    assert!(!results.to_string().contains(DUMMY_KEY));
    drop((app, engine));

    let engine = Arc::new(Engine::new(config).await.unwrap());
    let app = router(engine.clone());
    let stored = engine.indexer(&id).await.unwrap();
    assert_eq!(
        stored.definition,
        Some(json!({"single_file_release_use_filename": true}))
    );
    assert_eq!(stored.api_key, DUMMY_KEY);
    // Custom definitions and credentials stay out of the public listing.
    let (_, public) = call(&app, "GET", "/api/indexers", Value::Null).await;
    assert!(public[0].get("definition").is_none());
    assert!(!public.to_string().contains(DUMMY_KEY));
    let (status, results) = call(
        &app,
        "GET",
        &format!("/api/search?indexers={id}&q=restart"),
        Value::Null,
    )
    .await;
    assert!(status.is_success());
    assert_eq!(result_titles(&results), ENABLED_TITLES);

    let (status, _) = call(
        &app,
        "PATCH",
        &format!("/api/indexers/{id}"),
        json!({"definition":{"single_file_release_use_filename":false}}),
    )
    .await;
    assert!(status.is_success());
    let (_, results) = call(
        &app,
        "GET",
        &format!("/api/search?indexers={id}&q=disabled"),
        Value::Null,
    )
    .await;
    assert_eq!(result_titles(&results), DEFAULT_TITLES);

    let auth = up.auth.lock().unwrap();
    assert_eq!(auth.len(), 3);
    assert!(auth.iter().all(|a| a == &format!("Bearer {DUMMY_KEY}")));
}
