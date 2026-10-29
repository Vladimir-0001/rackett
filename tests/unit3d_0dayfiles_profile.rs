//! Offline native UNIT3D recipe for 0DayFiles: assets/adapter-profiles/0dayfiles-api.json.
//!
//! Expectations are derived from the unchanged vendored 0dayfiles-api.yml through the
//! existing Cardigann plan (pinned TorznabCapabilitiesCategories semantics), never
//! from the recipe itself. Rows are synthetic, shaped after the definition's `data[]
//! .attributes` fields; the upstream is loopback and the key is a dummy. The recipe
//! is a definition only: no endpoint, key or configured indexer, and it is not a
//! catalog preset.
use axum::{
    body::{to_bytes, Body},
    extract::RawQuery,
    http::{HeaderMap, Request, StatusCode},
    routing::get,
    Router,
};
use chrono::{TimeZone, Utc};
use rackett::{
    adapters,
    cardigann::{parse, Limits},
    cardigann_exec::Plan,
    engine::Engine,
    models::{Indexer, Release, SearchQuery},
    AppState,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};
use tower::ServiceExt;

const RECIPE: &str = include_str!("../assets/adapter-profiles/0dayfiles-api.json");
const SOURCE: &str = include_str!("../vendor/jackett/definitions/0dayfiles-api.yml");
const SOURCE_SHA256: &str = "08674266f79acc0f6fda52e44e4f32deafd9689042c7d9a6ade6a5fd1f421668";
const PIN: &str = "fe8876d21b93dfda84eb12bd24e1c72e56fb8736";
/// Tracker IDs of the pinned categorymappings, in definition order.
const SOURCE_IDS: [u32; 38] = [
    9, 10, 11, 12, 13, 14, 17, 18, 19, 20, 58, 21, 22, 23, 25, 26, 28, 29, 30, 31, 32, 59, 33, 34,
    38, 39, 41, 42, 43, 44, 46, 48, 49, 51, 1, 54, 55, 56,
];
const DUMMY_KEY: &str = "dummy-0dayfiles-fixture-key";
const TORRENT: &[u8] = b"d4:infod4:name7:fixture6:lengthi42eee";
const BASE: &str = "http://127.0.0.1:9117";

fn recipe() -> Value {
    serde_json::from_str(RECIPE).unwrap()
}
/// Recipe category map with tracker IDs as strings, matching Cardigann's `.Categories`.
fn recipe_map() -> BTreeMap<u32, Vec<String>> {
    recipe()["categories"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, ids)| {
            let ids = ids.as_array().unwrap();
            (
                k.parse().unwrap(),
                ids.iter()
                    .map(|v| v.as_u64().unwrap().to_string())
                    .collect(),
            )
        })
        .collect()
}
fn plan() -> Plan {
    Plan::compile(&parse(SOURCE, Limits::default()).unwrap()).unwrap()
}
fn query(pairs: &[(&str, &str)]) -> SearchQuery {
    SearchQuery {
        params: pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}
fn pinned_context(p: &Plan, q: &SearchQuery) -> rackett::cardigann_exec::template::Context {
    let settings = BTreeMap::from([("apikey".into(), json!(DUMMY_KEY))]);
    p.context(
        &settings,
        q,
        Utc.with_ymd_and_hms(2026, 10, 28, 0, 0, 0).unwrap(),
    )
}
/// Pinned MapTorznabCapsToTrackers for a `cat` value.
fn pinned_trackers(p: &Plan, cat: &str) -> Vec<String> {
    pinned_context(p, &query(&[("cat", cat)])).values[".Categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect()
}
/// Query pairs of the pinned definition's built search request.
fn pinned_request(p: &Plan, q: &SearchQuery) -> Vec<(String, String)> {
    let request = &p.build_requests(&pinned_context(p, q)).unwrap()[0];
    assert!(request
        .headers
        .contains(&("Authorization".into(), format!("Bearer {DUMMY_KEY}"))));
    url::Url::parse(&request.url)
        .unwrap()
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}
/// Pinned MapTrackerCatToNewznab for one synthetic row, through the vendored fields.
fn pinned_response(p: &Plan, category_id: Value) -> Vec<u32> {
    let ctx = pinned_context(p, &SearchQuery::default());
    let request = &p.build_requests(&ctx).unwrap()[0];
    // The plan does not model Newtonsoft's ISO date conversion, so this derivation row
    // carries the converted string that the vendored dateparse layout expects.
    let mut row = row(1, "https://0dayfiles.invalid/", category_id);
    row["attributes"]["created_at"] = json!("10/18/2021 00:34:50");
    let body = json!({"data": [row]}).to_string();
    p.extract(request, &body, &ctx).unwrap()[0]
        .categories
        .clone()
}
fn categories_param(pairs: &[(String, String)]) -> Vec<String> {
    pairs
        .iter()
        .filter(|(k, _)| k == "categories[]")
        .map(|(_, v)| v.clone())
        .collect()
}
fn parent(id: u32) -> u32 {
    id / 1000 * 1000
}

/// Synthetic modern UNIT3D row; download and details links are on `base`'s origin.
fn row(id: u32, base: &str, category_id: Value) -> Value {
    json!({"type":"torrent","id":id.to_string(),"attributes":{
        "name":format!("Fixture.Release.{id}"),"category_id":category_id,
        "download_link":format!("{base}torrent/download/{id}.dummy-rsskey"),
        "details_link":format!("{base}torrents/{id}"),
        "size":1024,"seeders":2,"leechers":1,"times_completed":3,
        "created_at":"2021-10-18T00:34:50.000000Z","freeleech":"0%",
        "double_upload":false,"featured":false,"internal":false,
        "num_file":1,"files":[{"name":format!("fixture.{id}.mkv")}],
        "meta":{"poster":"","genres":""}}})
}
/// One row per source tracker ID (alternating JSON number/string category_id),
/// then a row with an unmapped category and one without category_id.
fn rows(base: &str) -> Value {
    let mut data: Vec<Value> = SOURCE_IDS
        .iter()
        .enumerate()
        .map(|(n, &id)| {
            let cat = if n % 2 == 0 {
                json!(id)
            } else {
                json!(id.to_string())
            };
            row(id, base, cat)
        })
        .collect();
    data.push(row(900, base, json!(99999)));
    let mut missing = row(901, base, Value::Null);
    missing["attributes"]
        .as_object_mut()
        .unwrap()
        .remove("category_id");
    data.push(missing);
    json!({ "data": data })
}

type Log = Arc<Mutex<Vec<String>>>;
struct Upstream {
    url: String,
    queries: Log,
    auth: Log,
    downloads: Log,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Upstream {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Upstream {
    /// Category values of every recorded search request.
    fn sent_categories(&self) -> Vec<Vec<String>> {
        self.queries
            .lock()
            .unwrap()
            .iter()
            .map(|q| {
                let pairs: Vec<_> = url::form_urlencoded::parse(q.as_bytes())
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
                categories_param(&pairs)
            })
            .collect()
    }
}
async fn upstream() -> Upstream {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let (queries, auth, downloads): (Log, Log, Log) = Default::default();
    let (q, a, d, body) = (
        queries.clone(),
        auth.clone(),
        downloads.clone(),
        rows(&url).to_string(),
    );
    let bearer = |headers: &HeaderMap| {
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    let app = Router::new()
        .route(
            "/api/torrents/filter",
            get(move |RawQuery(raw): RawQuery, headers: HeaderMap| {
                q.lock().unwrap().push(raw.unwrap_or_default());
                a.lock().unwrap().push(bearer(&headers));
                let body = body.clone();
                async move { ([("content-type", "application/json")], body) }
            }),
        )
        .route(
            "/torrent/download/{file}",
            get(move |headers: HeaderMap| {
                d.lock().unwrap().push(bearer(&headers));
                async { TORRENT }
            }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Upstream {
        url,
        queries,
        auth,
        downloads,
        server,
    }
}
fn indexer(url: &str) -> Indexer {
    Indexer {
        id: "0dayfiles-fixture".into(),
        name: "0DayFiles fixture".into(),
        kind: "unit3d".into(),
        url: url.into(),
        api_key: DUMMY_KEY.into(),
        request_delay_ms: 0,
        definition: Some(recipe()),
        ..Default::default()
    }
}
async fn native(i: &Indexer, q: SearchQuery) -> Vec<Release> {
    adapters::search(&reqwest::Client::new(), i, &q)
        .await
        .unwrap()
}

#[test]
fn source_is_the_unchanged_pinned_definition() {
    assert_eq!(hex(&Sha256::digest(SOURCE)), SOURCE_SHA256);
    let sums = std::fs::read_to_string("vendor/jackett/SHA256SUMS").unwrap();
    assert!(sums
        .lines()
        .any(|l| l == format!("{SOURCE_SHA256}  definitions/0dayfiles-api.yml")));
    let notice = std::fs::read_to_string("vendor/jackett/NOTICE").unwrap();
    assert!(notice.contains(&format!("Pinned commit: {PIN}")));
    let ids: Vec<u32> = SOURCE
        .lines()
        .filter_map(|l| l.trim().strip_prefix("- {id: "))
        .map(|l| l.split(',').next().unwrap().parse().unwrap())
        .collect();
    assert_eq!(ids, SOURCE_IDS);
    // The compiled plan knows each tracker ID: each has its 100000+ custom category.
    let p = plan();
    for id in SOURCE_IDS {
        assert_eq!(
            pinned_trackers(&p, &(100_000 + id).to_string()),
            [id.to_string()]
        );
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn recipe_is_a_valid_definition_only_unit3d_profile() {
    let r = recipe();
    let keys: Vec<_> = r.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "categories",
            "keywords_replace_dots",
            "single_file_release_use_filename"
        ]
    );
    assert_eq!(r["keywords_replace_dots"], json!(true));
    assert_eq!(r["single_file_release_use_filename"], json!(true));
    for absent in ["apikey", "api_key", "http", "0dayfiles.net"] {
        assert!(!RECIPE.contains(absent), "{absent}");
    }
    adapters::validate(&indexer("http://127.0.0.1:9/")).unwrap();
    let source: BTreeSet<String> = SOURCE_IDS.iter().map(u32::to_string).collect();
    let map = recipe_map();
    assert_eq!(map.len(), 29);
    for (key, ids) in &map {
        assert!(*key < 100_000, "no custom categories");
        let unique: BTreeSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "{key} IDs are unique");
        assert!(ids.iter().all(|id| source.contains(id)), "{key}");
    }
    // Each source ID sits under its leaf key and, for child categories, the parent key.
    let mut uses = BTreeMap::<&str, usize>::new();
    for id in map.values().flatten() {
        *uses.entry(id).or_default() += 1;
    }
    assert_eq!(uses.len(), SOURCE_IDS.len());
    // Only the eight mappings straight to a parent category appear once.
    let once: Vec<&str> = uses
        .iter()
        .filter(|(_, n)| **n == 1)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(once, ["17", "28", "33", "38", "41", "46", "49", "54"]);
    assert!(uses.values().all(|n| *n <= 2));
}

#[test]
fn every_category_key_matches_pinned_request_mapping() {
    let (p, map) = (plan(), recipe_map());
    for cat in 0..10_000u32 {
        let native = map.get(&cat).cloned().unwrap_or_default();
        assert_eq!(native, pinned_trackers(&p, &cat.to_string()), "cat {cat}");
    }
    // Parent keys expand to every mapped child plus any direct parent mapping.
    let strings = |ids: &[u32]| ids.iter().map(u32::to_string).collect::<Vec<_>>();
    assert_eq!(
        map[&2000],
        strings(&[9, 10, 11, 12, 13, 14, 17, 18, 19, 20, 58])
    );
    assert_eq!(
        map[&4000],
        strings(&[34, 51, 1]),
        "PC has no direct mapping"
    );
    assert!(map[&5000].contains(&"43".into()) && map[&5000].contains(&"48".into()));
    for (key, ids) in &map {
        if parent(*key) == *key {
            let children: BTreeSet<_> = map
                .iter()
                .filter(|(k, _)| *k != key && parent(**k) == *key)
                .flat_map(|(_, ids)| ids)
                .collect();
            assert!(children.iter().all(|id| ids.contains(id)), "{key}");
        }
    }
    // Configured capabilities: the recipe keys are the pinned tree minus custom IDs.
    // The plan omits a parent known only through children (PC); upstream's tree has it.
    let mut tree: BTreeSet<u32> = p.supported_categories().into_iter().collect();
    tree.extend(tree.clone().iter().map(|c| parent(*c)));
    assert_eq!(map.keys().copied().collect::<BTreeSet<_>>(), tree);
    // Known divergence: no 100000+ custom categories are mapped.
    for id in SOURCE_IDS {
        assert!(!map.contains_key(&(100_000 + id)));
    }
}

#[tokio::test]
async fn loopback_requests_send_raw_tracker_categories() {
    let (p, map) = (plan(), recipe_map());
    let up = upstream().await;
    let i = indexer(&up.url);
    for key in map.keys() {
        native(&i, query(&[("cat", &key.to_string())])).await;
    }
    let sent = up.sent_categories();
    for (n, key) in map.keys().enumerate() {
        let pinned = categories_param(&pinned_request(&p, &query(&[("cat", &key.to_string())])));
        assert_eq!(sent[n], pinned, "cat {key}");
    }
    up.queries.lock().unwrap().clear();
    // RequestPlan models mapping only: empty/unknown categories yield no filter.
    // Jackett separately skips wholly unsupported categories before making a request.
    for cat in ["", ",", "9999", "2010", "100009"] {
        native(&i, query(&[("cat", cat)])).await;
    }
    native(&i, SearchQuery::default()).await;
    assert!(up.sent_categories().iter().all(Vec::is_empty));
    // Except 100009: upstream maps its custom category to tracker 9; the recipe does not.
    assert_eq!(pinned_trackers(&p, "100009"), ["9"]);
    up.queries.lock().unwrap().clear();
    // Characterized: overlapping/duplicate query categories repeat IDs; upstream is distinct.
    for cat in ["2000,2030", "2030,2030", "2030,2000"] {
        native(&i, query(&[("cat", cat)])).await;
    }
    let sent = up.sent_categories();
    let concat = |cats: &[u32]| cats.iter().flat_map(|c| map[c].clone()).collect::<Vec<_>>();
    assert_eq!(sent[0], concat(&[2000, 2030]));
    assert_eq!(sent[1], concat(&[2030, 2030]));
    assert_eq!(sent[2], concat(&[2030, 2000]));
    for (n, cat) in ["2000,2030", "2030,2030", "2030,2000"].iter().enumerate() {
        let distinct: BTreeSet<_> = sent[n].iter().cloned().collect();
        let pinned = pinned_trackers(&p, cat);
        assert_eq!(distinct, pinned.iter().cloned().collect::<BTreeSet<_>>());
        assert!(sent[n].len() > pinned.len(), "{cat} repeats IDs natively");
    }
    assert!(up
        .auth
        .lock()
        .unwrap()
        .iter()
        .all(|a| a == &format!("Bearer {DUMMY_KEY}")));
}

#[tokio::test]
async fn loopback_query_and_id_parameters_match_pinned_inputs() {
    let p = plan();
    let up = upstream().await;
    let i = indexer(&up.url);
    let ids = query(&[
        ("t", "tvsearch"),
        ("q", "Fixture Show"),
        ("season", "2"),
        ("ep", "3"),
        ("imdbid", "tt0133093"),
        ("tmdbid", "1396"),
        ("tvdbid", "81189"),
    ]);
    let dotted = query(&[("q", "Fixture.Show")]);
    native(&i, ids.clone()).await;
    native(&i, dotted.clone()).await;
    let sent: Vec<BTreeMap<String, String>> = up
        .queries
        .lock()
        .unwrap()
        .iter()
        .map(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .into_owned()
                .collect()
        })
        .collect();
    let pinned: BTreeMap<_, _> = pinned_request(&p, &ids).into_iter().collect();
    for key in [
        "name",
        "seasonNumber",
        "episodeNumber",
        "imdbId",
        "tmdbId",
        "tvdbId",
        "sortField",
        "sortDirection",
        "perPage",
    ] {
        assert_eq!(sent[0].get(key), pinned.get(key), "{key}");
    }
    assert_eq!(sent[0]["name"], "Fixture Show S02E03");
    // Neither side sends free[] with the default freeleech=false.
    assert!(!sent[0].contains_key("free[]") && !pinned.contains_key("free[]"));
    // The profile enables the pinned dotted-keyword filter.
    let pinned: BTreeMap<_, _> = pinned_request(&p, &dotted).into_iter().collect();
    assert_eq!(pinned["name"], "Fixture Show");
    assert_eq!(sent[1]["name"], pinned["name"]);
    for title in ["Fixture.Show", "Fixture..Show", "Fixture.Show."] {
        let q = query(&[("q", title), ("season", "2"), ("ep", "3")]);
        native(&i, q.clone()).await;
        let raw = up.queries.lock().unwrap().last().unwrap().clone();
        let actual: BTreeMap<_, _> = url::form_urlencoded::parse(raw.as_bytes())
            .into_owned()
            .collect();
        let expected: BTreeMap<_, _> = pinned_request(&p, &q).into_iter().collect();
        assert_eq!(actual["name"], expected["name"], "{title}");
    }
}

#[tokio::test]
async fn response_categories_reverse_to_leaf_and_parent() {
    let (p, map) = (plan(), recipe_map());
    let up = upstream().await;
    let mut i = indexer(&up.url);
    let releases = native(&i, SearchQuery::default()).await;
    assert_eq!(releases.len(), SOURCE_IDS.len() + 2);
    for (release, id) in releases.iter().zip(SOURCE_IDS) {
        let pinned = pinned_response(&p, json!(id));
        assert_eq!(pinned_response(&p, json!(id.to_string())), pinned);
        // Upstream returns the leaf plus the 100000+ custom category, never the parent.
        let standard: Vec<u32> = pinned.iter().copied().filter(|c| *c < 100_000).collect();
        assert_eq!(standard.len(), 1, "{id}");
        assert_eq!(pinned, [standard[0], 100_000 + id]);
        // Native reverse lookup returns every key listing the ID: leaf and parent.
        let expected: BTreeSet<u32> = [standard[0], parent(standard[0])].into();
        assert_eq!(
            release.categories,
            expected.into_iter().collect::<Vec<_>>(),
            "{id}"
        );
        assert_eq!(release.title, format!("fixture.{id}.mkv"));
    }
    assert_eq!(releases[0].categories, [2000, 2030], "tracker 9");
    assert_eq!(
        releases[6].categories,
        [2000],
        "tracker 17 maps to the parent only"
    );
    // Unmapped or missing IDs stay uncategorized even with advertised categories.
    assert_eq!(pinned_response(&p, json!(99999)), Vec::<u32>::new());
    assert!(releases[38].categories.is_empty() && releases[39].categories.is_empty());
    i.categories = map.keys().copied().collect();
    let releases = native(&i, SearchQuery::default()).await;
    assert!(releases[38].categories.is_empty());
    assert!(releases[39].categories.is_empty());
    assert_eq!(releases[0].categories, [2000, 2030]);
    // Without an explicit map, the existing generic adapter fallback is unchanged.
    i.definition = None;
    let releases = native(&i, SearchQuery::default()).await;
    assert!(releases.iter().all(|r| r.categories == i.categories));
}

async fn call(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Vec<u8>) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1:9117")
        .header("origin", BASE)
        .header("content-type", "application/json")
        .body(if body.is_null() {
            Body::empty()
        } else {
            Body::from(body.to_string())
        })
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (status, bytes.to_vec())
}
async fn call_json(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let (status, bytes) = call(app, method, path, body).await;
    (status, serde_json::from_slice(&bytes).unwrap())
}
fn app(engine: &Arc<Engine>) -> Router {
    Router::new()
        .merge(rackett::api::router())
        .merge(rackett::protocol::router())
        .with_state(AppState {
            engine: engine.clone(),
        })
}
/// Torznab items as (title, categories, enclosure URL).
fn items(xml: &[u8]) -> Vec<(String, Vec<u32>, String)> {
    let text = std::str::from_utf8(xml).unwrap();
    let doc = roxmltree::Document::parse(text).unwrap();
    doc.descendants()
        .filter(|n| n.has_tag_name("item"))
        .map(|item| {
            let child = |name: &str| item.children().find(|n| n.has_tag_name(name));
            let cats = item
                .children()
                .filter(|n| {
                    n.tag_name().name() == "attr" && n.attribute("name") == Some("category")
                })
                .map(|n| n.attribute("value").unwrap().parse().unwrap())
                .collect();
            (
                child("title").unwrap().text().unwrap().to_owned(),
                cats,
                child("enclosure")
                    .unwrap()
                    .attribute("url")
                    .unwrap()
                    .to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn engine_create_restart_test_search_caps_and_same_origin_download() {
    let map = recipe_map();
    let configured: Vec<u32> = map.keys().copied().collect();
    let up = upstream().await;
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.json");
    let engine = Arc::new(Engine::new(config.clone()).await.unwrap());
    let (status, created) = call_json(
        &app(&engine),
        "POST",
        "/api/indexers",
        json!({"name":"0DayFiles fixture","kind":"unit3d","url":up.url,"api_key":DUMMY_KEY,
               "request_delay_ms":0,"categories":configured,"definition":recipe()}),
    )
    .await;
    assert!(status.is_success(), "{created}");
    assert!(!created.to_string().contains(DUMMY_KEY));
    let id = created["id"].as_str().unwrap().to_owned();
    drop(engine);

    let engine = Arc::new(Engine::new(config).await.unwrap());
    let app = app(&engine);
    let stored = engine.indexer(&id).await.unwrap();
    assert_eq!(stored.definition, Some(recipe()));
    assert_eq!(stored.categories, configured);
    let (_, public) = call_json(&app, "GET", "/api/indexers", Value::Null).await;
    assert!(public[0].get("definition").is_none());
    assert!(!public.to_string().contains(DUMMY_KEY));

    let (_, tested) = call_json(
        &app,
        "POST",
        &format!("/api/indexers/{id}/test"),
        Value::Null,
    )
    .await;
    assert_eq!(tested["ok"], json!(true), "{tested}");

    let key = engine.settings().await.api_key;
    let (status, caps) = call(
        &app,
        "GET",
        &format!("/torznab/{id}/api?t=caps&apikey={key}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let caps = String::from_utf8(caps).unwrap();
    let doc = roxmltree::Document::parse(&caps).unwrap();
    let advertised: Vec<u32> = doc
        .descendants()
        .filter(|n| n.has_tag_name("category"))
        .map(|n| n.attribute("id").unwrap().parse().unwrap())
        .collect();
    assert_eq!(advertised, configured);
    // Native UNIT3D caps do not advertise the definition's ID parameters, although an
    // explicitly named source accepts and forwards imdbid/tmdbid/tvdbid.
    let movie = doc
        .descendants()
        .find(|n| n.has_tag_name("movie-search"))
        .unwrap();
    assert_eq!(
        movie.attribute("supportedParams"),
        Some("q,imdbid,tmdbid,tvdbid")
    );
    let (status, _) = call(
        &app,
        "GET",
        &format!("/torznab/{id}/api?t=movie&imdbid=tt0133093&cache=false&apikey={key}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(up
        .queries
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .contains("imdbId=0133093"));

    let (status, xml) = call(
        &app,
        "GET",
        &format!("/torznab/{id}/api?t=search&cat=5070&cache=false&apikey={key}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let found = items(&xml);
    assert_eq!(
        up.sent_categories().last().unwrap(),
        &["43".to_owned()],
        "only TV/Anime is requested"
    );
    // The synthetic upstream ignores filters; engine filtering keeps tracker 43 plus the
    // uncategorized rows, matching Jackett's category-filter behavior.
    let titles: Vec<_> = found.iter().map(|(t, _, _)| t.as_str()).collect();
    assert_eq!(
        titles,
        ["fixture.43.mkv", "fixture.900.mkv", "fixture.901.mkv"]
    );
    assert_eq!(found[0].1, [5000, 5070]);
    assert!(found[1].1.is_empty() && found[2].1.is_empty());
    assert!(!String::from_utf8_lossy(&xml).contains(DUMMY_KEY));

    // Upstream's custom category 100009 means tracker 9; natively it sends no filter and
    // the engine filter drops every categorized row.
    let (status, xml) = call(
        &app,
        "GET",
        &format!("/torznab/{id}/api?t=search&cat=100009&cache=false&apikey={key}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        items(&xml)
            .iter()
            .map(|(t, _, _)| t.as_str())
            .collect::<Vec<_>>(),
        ["fixture.900.mkv", "fixture.901.mkv"]
    );
    assert!(up.sent_categories().last().unwrap().is_empty());

    let link = url::Url::parse(&found[0].2).unwrap();
    assert!(link.as_str().starts_with(&format!("{BASE}/dl/")), "{link}");
    let (status, bytes) = call(
        &app,
        "GET",
        &format!("{}?{}", link.path(), link.query().unwrap()),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(bytes, TORRENT);
    assert_eq!(
        *up.downloads.lock().unwrap(),
        [format!("Bearer {DUMMY_KEY}")]
    );
    assert!(up
        .auth
        .lock()
        .unwrap()
        .iter()
        .all(|a| a == &format!("Bearer {DUMMY_KEY}")));
}
