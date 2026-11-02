//! Independent, source-pinned HTML layouts and full form-login lifecycles.
//! Synthetic fixtures establish runtime support, never live account availability.
use rackett::{builtin, cardigann, cardigann_exec::Plan, models::SearchQuery};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// A `download.selectors` source: its release link is a detail page whose selector
/// leads to `resolved`, a same-origin torrent path or an expected magnet URI.
#[derive(Clone, Deserialize)]
struct Detail {
    html: String,
    resolved: String,
}
#[derive(Clone, Deserialize)]
struct Case {
    id: String,
    /// The search response is JSON (`response: {type: json}`): the fixture file is the
    /// JSON body (no rows markers) and `empty_body` is the source's empty result.
    #[serde(default)]
    json: bool,
    #[serde(default)]
    empty_body: Option<String>,
    #[serde(default)]
    detail: Option<Detail>,
    /// Extra markup on the login landing page after the form, e.g. the token a
    /// `login.selectorinputs` entry copies. The copied value must also appear in
    /// `expected_inputs` under its input name.
    #[serde(default)]
    landing_html: Option<String>,
    /// A `login.cookies` seed (e.g. `JAVA=OK`) the landing request must carry.
    #[serde(default)]
    landing_cookie: Option<String>,
    /// The definition searches with `method: post`; keyword and category pairs are
    /// matched in the form body instead of the query string.
    #[serde(default)]
    search_post: bool,
    /// A `download.before` request path (e.g. `/thanks.php`); the fixture answers it
    /// with 200. `before_fields` must all be present in its POST body.
    #[serde(default)]
    before_path: Option<String>,
    #[serde(default)]
    before_fields: BTreeMap<String, String>,
    #[serde(default)]
    direct_post: bool,
    /// The login identity setting is not called `username` (magnetz uses `email`).
    #[serde(default)]
    user_setting: Option<String>,
    /// The definition declares no settings and logs in without credentials (comicat and
    /// miobt send a literal `visitor_test` input). The password-change step is skipped.
    #[serde(default)]
    no_credentials: bool,
    /// The definition declares this literal `Cookie` search header (eztvl). Pinned Jackett
    /// sends declared headers on every request, so each request must carry it, alone or
    /// followed by `"; "` and the session cookie (docs/session-search-headers.md).
    #[serde(default)]
    literal_cookie: Option<String>,
    /// The definition declares no `login.test`: pinned TestLogin sends no validation
    /// request and only a search redirect means expiry (docs/login-without-test.md).
    /// `login_path` then names a path the session must never request.
    #[serde(default)]
    no_test: bool,
    sha256: String,
    search_fixture: String,
    form_path: String,
    submit_path: String,
    form_id: String,
    form_attributes: Option<String>,
    expected_title: Option<String>,
    expected_size: Option<u64>,
    expected_seeders: Option<u64>,
    expected_peers: Option<u64>,
    timeout_seconds: Option<u64>,
    expected_inputs: BTreeMap<String, String>,
    #[serde(default)]
    configured_settings: BTreeMap<String, String>,
    /// Declared non-secret settings sent with the credentials and used for the offline
    /// extraction context, e.g. animelayer's `category-id` select in its search path.
    /// The catalog must list each as not secret; unlike `configured_settings` they are
    /// not leak-checked, since public definitions show them.
    #[serde(default)]
    public_settings: BTreeMap<String, String>,
    error_html: String,
    search_error_html: Option<String>,
    category: u32,
    custom_category: Option<u32>,
    missing_title: Option<String>,
    request_count: usize,
    login_path: String,
    search_path: String,
    /// Query or body key that carries `keyword_value`. Empty when the definition has
    /// no search inputs and its path carries the keyword (torrentleech); the exact
    /// `search_path` then identifies a search request.
    keyword: String,
    keyword_value: String,
    category_input: Option<[String; 2]>,
    download_path: String,
    /// Expected details link suffix. Absent when the definition declares no `details`
    /// field (acrossthetasman): releases must then carry no details link.
    #[serde(default)]
    details_path: Option<String>,
    published: String,
    free_markup: String,
    normal_markup: String,
    free_download: f64,
    free_upload: f64,
    /// Upload factor of a non-promoted row when the source declares one (default 1).
    normal_upload: Option<f64>,
    normal_download: Option<f64>,
}
impl Case {
    fn user_key(&self) -> &str {
        self.user_setting.as_deref().unwrap_or("username")
    }
}
/// Independently reviewed admission batches; each file uses the same `Case` schema.
const FORM_BATCHES: [&str; 24] = [
    include_str!("fixtures/form-batches/batch1.json"),
    include_str!("fixtures/form-batches/batch2.json"),
    include_str!("fixtures/form-batches/batch3.json"),
    include_str!("fixtures/form-batches/batch4.json"),
    include_str!("fixtures/form-batches/batch5.json"),
    include_str!("fixtures/form-batches/batch6.json"),
    include_str!("fixtures/form-batches/batch7.json"),
    include_str!("fixtures/form-batches/batch8.json"),
    include_str!("fixtures/form-batches/batch9.json"),
    include_str!("fixtures/form-batches/batch10.json"),
    include_str!("fixtures/form-batches/batch11.json"),
    include_str!("fixtures/form-batches/batch12.json"),
    include_str!("fixtures/form-batches/batch13.json"),
    include_str!("fixtures/form-batches/batch14.json"),
    include_str!("fixtures/form-batches/batch15.json"),
    include_str!("fixtures/form-batches/batch16.json"),
    include_str!("fixtures/form-batches/batch17.json"),
    include_str!("fixtures/form-batches/batch18.json"),
    include_str!("fixtures/form-batches/batch19.json"),
    include_str!("fixtures/form-batches/batch20.json"),
    include_str!("fixtures/form-batches/batch21.json"),
    include_str!("fixtures/form-batches/batch22.json"),
    include_str!("fixtures/form-batches/batch23.json"),
    include_str!("fixtures/form-batches/batch24.json"),
];
/// Whole-lifecycle budget: five times a case's declared minimum request timeout, at
/// least 300 s. Slow pinned sources (multi-page searches with request delays) declare
/// `timeout_seconds` (see docs/session-validation.md).
fn lifecycle_budget(c: &Case) -> std::time::Duration {
    std::time::Duration::from_secs(c.timeout_seconds.unwrap_or(0).saturating_mul(5).max(300))
}
fn cases() -> Vec<Case> {
    let mut cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/form-layouts/cases.json")).unwrap();
    cases.extend(
        serde_json::from_str::<Vec<Case>>(include_str!("fixtures/post-layouts/cases.json"))
            .unwrap(),
    );
    for batch in FORM_BATCHES {
        cases.extend(serde_json::from_str::<Vec<Case>>(batch).unwrap());
    }
    // `RACKETT_FORM_CASES=pttime,jpopsuki` re-runs single lifecycles locally.
    if let Ok(only) = std::env::var("RACKETT_FORM_CASES") {
        let only: Vec<&str> = only.split(',').collect();
        cases.retain(|c| only.contains(&c.id.as_str()));
    }
    cases
}
fn credentials(c: &Case, user: &str, password: &str) -> serde_json::Value {
    let mut settings = c.public_settings.clone();
    settings.extend(c.configured_settings.clone());
    if !c.no_credentials {
        settings.insert(c.user_key().into(), user.into());
        settings.insert("password".into(), password.into());
    }
    json!(settings)
}
fn root(c: &Case) -> cardigann::Node {
    let raw = builtin::SOURCES
        .iter()
        .find(|(id, _)| *id == c.id)
        .unwrap()
        .1;
    assert_eq!(format!("{:x}", Sha256::digest(raw)), c.sha256, "{}", c.id);
    cardigann::parse(raw, cardigann::Limits::default()).unwrap()
}
fn page(c: &Case, free: bool) -> String {
    let file = if let Some(id) = c.search_fixture.strip_prefix("post:") {
        format!("tests/fixtures/post-layouts/{id}.html")
    } else {
        match c.search_fixture.strip_prefix("form:") {
            Some(id) => format!("tests/fixtures/form-layouts/{id}.html"),
            None => format!("tests/fixtures/cookie-layouts/{}.html", c.search_fixture),
        }
    };
    std::fs::read_to_string(file).unwrap().replace(
        "@@PROMO@@",
        if free {
            &c.free_markup
        } else {
            &c.normal_markup
        },
    )
}
fn empty_for(c: &Case, page: &str) -> String {
    match (&c.empty_body, c.json) {
        (Some(body), true) => body.clone(),
        _ => empty(page),
    }
}
fn empty(page: &str) -> String {
    let (before, rest) = page.split_once("<!-- rows:start -->").unwrap();
    let (_, after) = rest.split_once("<!-- rows:end -->").unwrap();
    format!("{before}{after}")
}
fn query(c: &Case) -> SearchQuery {
    SearchQuery {
        params: BTreeMap::from([
            ("q".into(), "Fixture".into()),
            ("cat".into(), c.category.to_string()),
        ]),
    }
}
/// Parses a form body in order, keeping repeated keys. A non-UTF-8 body must use
/// HttpUtility.UrlEncode's lower-case escapes; its bytes decode with the definition
/// charset (WebClient.FormUrlEncodedContentWithEncoding).
fn form_pairs(
    c: &Case,
    charset: rackett::cardigann_exec::request::Charset,
    body: &[u8],
) -> Vec<(String, String)> {
    if charset == rackett::cardigann_exec::request::Charset::Utf8 {
        return url::form_urlencoded::parse(body).into_owned().collect();
    }
    let text = std::str::from_utf8(body).expect("percent-encoded body is ASCII");
    for escape in text.split('%').skip(1) {
        assert!(
            escape.len() >= 2
                && escape[..2]
                    .bytes()
                    .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
            "{}: non-HttpUtility escape in {text}",
            c.id
        );
    }
    let decode = |part: &str| {
        let raw = part.replace('+', " ");
        let mut bytes = Vec::new();
        let mut i = 0;
        while i < raw.len() {
            if raw.as_bytes()[i] == b'%' {
                bytes.push(u8::from_str_radix(&raw[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                bytes.push(raw.as_bytes()[i]);
                i += 1;
            }
        }
        charset.decode(bytes).unwrap()
    };
    text.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (decode(k), decode(v))
        })
        .collect()
}
/// Re-serializes decoded pairs as a UTF-8 query so search matching sees one shape.
fn query_text(pairs: &[(String, String)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish()
}
fn request_matches(c: &Case, url: &url::Url) {
    assert_eq!(url.path(), c.search_path, "{}", c.id);
    let pairs: Vec<_> = url.query_pairs().collect();
    assert!(
        c.keyword.is_empty() && url.path().contains(c.keyword_value.as_str())
            || pairs
                .iter()
                .any(|(k, v)| k == c.keyword.as_str() && v == c.keyword_value.as_str()),
        "{}: keyword",
        c.id
    );
    if let Some([key, value]) = &c.category_input {
        assert!(
            pairs
                .iter()
                .any(|(k, v)| k == key.as_str() && v == value.as_str()),
            "{}: category {key}={value}",
            c.id
        );
    }
    assert!(!url.as_str().contains("fixture-cookie"));
}
#[test]
fn pinned_form_layouts_extract_fields_promotions_and_empty_results() {
    let mut failures = Vec::new();
    for c in cases() {
        let id = c.id.clone();
        if std::panic::catch_unwind(|| check_layout(c)).is_err() {
            failures.push(id);
        }
    }
    assert!(failures.is_empty(), "Failed source layouts: {failures:?}");
}
fn check_layout(c: Case) {
    let p = Plan::compile(&root(&c)).unwrap_or_else(|e| panic!("{}: {e}", c.id));
    let now = "2026-10-29T12:00:00Z".parse().unwrap();
    let public = c
        .public_settings
        .iter()
        .map(|(k, v)| (k.clone(), json!(v)))
        .collect();
    let ctx = p.context(&public, &query(&c), now);
    let requests = p.build_requests(&ctx).unwrap();
    assert_eq!(requests.len(), c.request_count, "{}", c.id);
    let request = &requests[0];
    let mut url = url::Url::parse(&request.url).unwrap();
    if c.search_post {
        assert_eq!(request.method, "POST", "{}", c.id);
        let body = query_text(&form_pairs(
            &c,
            p.charset(),
            request.body.as_deref().unwrap_or_default().as_bytes(),
        ));
        let query = url.query().map(|q| format!("{q}&{body}")).unwrap_or(body);
        url.set_query(Some(&query));
    }
    request_matches(&c, &url);
    if let Some(error_html) = &c.search_error_html {
        let error = p
            .extract(request, &(page(&c, false) + error_html), &ctx)
            .unwrap_err();
        assert_eq!(error.path, "$.search.error[0]");
        assert!(!error.to_string().contains("fixture-private-error"));
    }
    for free in [false, true] {
        let body = page(&c, free);
        let rows = p
            .extract(request, &body, &ctx)
            .unwrap_or_else(|e| panic!("{} free={free}: {e}", c.id));
        assert_eq!(rows.len(), 1, "{}", c.id);
        let r = &rows[0];
        assert_eq!(
            r.title,
            c.expected_title.as_deref().unwrap_or("Fixture Release"),
            "{}",
            c.id
        );
        assert_eq!(r.size, Some(c.expected_size.unwrap_or(1 << 30)), "{}", c.id);
        assert_eq!(
            (r.seeders, r.peers),
            (
                Some(c.expected_seeders.unwrap_or(11)),
                Some(c.expected_peers.unwrap_or(15))
            ),
            "{}",
            c.id
        );
        assert!(r.categories.contains(&c.category), "{}", c.id);
        if let Some(category) = c.custom_category {
            assert!(r.categories.contains(&category), "{}", c.id);
        }
        assert_eq!(
            r.published.as_deref(),
            Some(c.published.as_str()),
            "{}",
            c.id
        );
        if c.download_path == "magnet" {
            const HASH: &str = "0123456789abcdef0123456789abcdef01234567";
            assert!(
                r.link.starts_with("magnet:?") && r.link.contains(HASH)
                    || r.attributes.get("infohash").is_some_and(|v| v == HASH),
                "{}: {:?}",
                c.id,
                r
            );
        } else {
            assert!(r.link.ends_with(&c.download_path), "{}: download", c.id);
        }
        match &c.details_path {
            Some(path) => assert!(
                r.details.as_deref().unwrap().ends_with(path),
                "{}: details",
                c.id
            ),
            None => assert_eq!(r.details, None, "{}: undeclared details", c.id),
        }
        assert_eq!(
            r.attributes
                .get("downloadvolumefactor")
                .map(|v| v.parse::<f64>().unwrap())
                .unwrap_or(1.0),
            if free {
                c.free_download
            } else {
                c.normal_download.unwrap_or(1.0)
            },
            "{}",
            c.id
        );
        assert_eq!(
            r.attributes
                .get("uploadvolumefactor")
                .map(|v| v.parse::<f64>().unwrap())
                .unwrap_or(1.0),
            if free {
                c.free_upload
            } else {
                c.normal_upload.unwrap_or(1.0)
            },
            "{}",
            c.id
        );
        assert!(
            p.extract(request, &empty_for(&c, &body), &ctx)
                .unwrap()
                .is_empty(),
            "{}",
            c.id
        );
        let mut untitled = body
            .replace("Fixture Release", "")
            .replace("Fixture%20Release", "");
        if let Some(title) = &c.expected_title {
            untitled = untitled.replace(title.as_str(), "");
        }
        let no_title = p.extract(request, &untitled, &ctx);
        if let Some(fallback) = &c.missing_title {
            assert_eq!(no_title.unwrap()[0].title, *fallback, "{}", c.id);
        } else if let Ok(rows) = no_title {
            // An error, or a row a declared row filter (andmatch) drops, both keep an
            // untitled release out of results.
            assert!(rows.is_empty(), "{}: empty title accepted", c.id);
        }
    }
}

#[test]
fn deferred_form_layouts_remain_unadmitted_with_reproducible_blockers() {
    let mut deferred: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/form-layouts/deferred.json")).unwrap();
    deferred.extend(
        serde_json::from_str::<Vec<Case>>(include_str!(
            "fixtures/form-layouts/deferred-animelayer.json"
        ))
        .unwrap(),
    );
    for c in deferred {
        assert!(["hdtorrents", "animelayer"].contains(&c.id.as_str()));
        assert!(!builtin::FORM_IDS.contains(&c.id.as_str()));
        let catalog = builtin::catalog();
        assert_eq!(
            catalog.iter().find(|r| r["id"] == c.id).unwrap()["ready"],
            false
        );
        let p = Plan::compile(&root(&c)).unwrap();
        let public = c
            .public_settings
            .iter()
            .map(|(k, v)| (k.clone(), json!(v)))
            .collect();
        let ctx = p.context(&public, &query(&c), "2026-10-29T12:00:00Z".parse().unwrap());
        let request = p.build_requests(&ctx).unwrap().remove(0);
        request_matches(&c, &url::Url::parse(&request.url).unwrap());
        // hdtorrents: pinned layout is HH:mm:ss<two spaces>dd/MM/yyyy. HTML extraction
        // normalizes the text to one space; dateparse leaves it unchanged and the
        // bounded fuzzy parser rejects it. animelayer: its month replacements yield
        // `28 October 2026 13:30` with no dateparse, also outside the fuzzy subset.
        // Keep the actual definitions intact.
        let error = p.extract(&request, &page(&c, false), &ctx).unwrap_err();
        assert_eq!(error.path, "$.search.fields.date");
        assert!(error.to_string().contains("unsupported fuzzy date syntax"));
    }
}

#[test]
fn additional_form_layouts_preserve_source_specific_filters_and_row_merges() {
    let cases = cases();
    for id in [
        "diablotorrent",
        "indietorrents",
        "mesevilag",
        "riperam",
        "bluraytracker",
    ] {
        let c = cases.iter().find(|c| c.id == id).unwrap();
        let plan = Plan::compile(&root(c)).unwrap();
        let now = "2026-10-29T12:00:00Z".parse().unwrap();
        let context = plan.context(&BTreeMap::new(), &query(c), now);
        let request = plan.build_requests(&context).unwrap().remove(0);
        let rows = plan.extract(&request, &page(c, true), &context).unwrap();
        match id {
            "diablotorrent" => {
                assert_eq!(
                    rows[0].attributes["description"],
                    "Verified: Fixture description"
                );
                let mut q = query(c);
                q.params.insert("cat".into(), "100034".into());
                let requests = plan
                    .build_requests(&plan.context(&BTreeMap::new(), &q, now))
                    .unwrap();
                assert_eq!(requests.len(), 1);
                assert_eq!(
                    url::Url::parse(&requests[0].url).unwrap().path(),
                    "/browse/1"
                );
                assert!(url::Url::parse(&requests[0].url)
                    .unwrap()
                    .query_pairs()
                    .any(|(k, v)| k == "category[]" && v == "34"));
                let missing_partner =
                    page(c, true).replace("class=\"torrent_details\"", "class=\"not-selected\"");
                assert_eq!(
                    plan.extract(&request, &missing_partner, &context)
                        .unwrap_err()
                        .path,
                    "$.search.rows.after"
                );
            }
            "indietorrents" => {
                assert_eq!(
                    rows[0].attributes["description"],
                    "2026 Digital</br>indie, pop"
                );
                let missing_partner =
                    page(c, true).replace("class=\"group_edition\"", "class=\"not-selected\"");
                assert_eq!(
                    plan.extract(&request, &missing_partner, &context)
                        .unwrap_err()
                        .path,
                    "$.search.rows.after"
                );
            }
            "mesevilag" => {
                assert_eq!(rows[0].attributes["description"], "Verified");
                let mut q = query(c);
                q.params.insert("q".into(), "Fixture..Release!".into());
                let requests = plan
                    .build_requests(&plan.context(&BTreeMap::new(), &q, now))
                    .unwrap();
                assert!(url::Url::parse(&requests[0].url)
                    .unwrap()
                    .query_pairs()
                    .any(|(k, v)| k == "search" && v == "Fixture%Release%"));
            }
            "riperam" => {
                let body =
                    page(c, true).replace("Fixture Release", "Fixture Release Сезон 1 Серия 2");
                assert_eq!(
                    plan.extract(&request, &body, &context).unwrap()[0].title,
                    "Fixture Release S1E2"
                );
                let mut q = query(c);
                q.params.insert("q".into(), "Fixture S01E02".into());
                let requests = plan
                    .build_requests(&plan.context(&BTreeMap::new(), &q, now))
                    .unwrap();
                assert!(url::Url::parse(&requests[0].url)
                    .unwrap()
                    .query_pairs()
                    .any(|(k, v)| k == "keywords" && v.trim() == "Fixture"));
            }
            "bluraytracker" => {
                let body = empty(&page(c, false))
                    + "<div class=\"error\">There are no results found.</div>";
                assert!(plan.extract(&request, &body, &context).unwrap().is_empty());
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn form_admission_matches_the_fixture_manifest_and_keeps_secrets_private() {
    let cases = cases();
    let ids = |post: bool| {
        let mut ids: Vec<_> = cases
            .iter()
            .filter(|c| c.direct_post == post)
            .map(|c| c.id.as_str())
            .collect();
        ids.sort_unstable();
        ids
    };
    assert_eq!(ids(false), builtin::FORM_IDS);
    assert_eq!(ids(true), builtin::POST_IDS);
    for c in cases {
        let root = root(&c);
        if c.direct_post {
            let errors = root
                .get("login")
                .unwrap()
                .get("error")
                .unwrap()
                .items()
                .unwrap();
            assert!(
                errors
                    .iter()
                    .any(|e| rackett::cardigann_exec::query_selector_exists(
                        &c.error_html,
                        e.string("selector").unwrap()
                    )
                    .unwrap()),
                "{}: error fixture must match its pinned selector",
                c.id
            );
        }
        assert!(
            rackett::builtin_runtime::form_auth::FormPlan::compile(&root).is_ok(),
            "{}",
            c.id
        );
        let row = builtin::catalog()
            .iter()
            .find(|row| row["id"] == c.id)
            .unwrap();
        assert_eq!(row["ready"], true, "{}: {}", c.id, row["blocked_reason"]);
        let identity: &[&str] = if c.no_credentials {
            &[]
        } else {
            &[c.user_key(), "password"]
        };
        for field in identity
            .iter()
            .copied()
            .chain(c.configured_settings.keys().map(String::as_str))
        {
            assert!(row["settings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["name"] == field && s["secret"] == true));
        }
        for field in c.public_settings.keys() {
            assert!(
                row["settings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|s| s["name"] == field.as_str() && s["secret"] == false),
                "{}: {field} must be a declared non-secret setting",
                c.id
            );
        }
        let base = "https://fixture.example/";
        let mut i = rackett::models::Indexer {
            kind: "cardigann".into(),
            url: base.into(),
            definition: Some(
                json!({"builtin_id":c.id,"settings":credentials(&c, "fixture-user", "fixture-password")}),
            ),
            ..Default::default()
        };
        assert!(builtin::Runtime::compile(&i).is_ok(), "{}", c.id);
        assert!(!builtin::public_definition(&i)
            .unwrap()
            .to_string()
            .contains("fixture-password"));
        if !c.no_credentials {
            i.definition.as_mut().unwrap()["settings"]["password"] = json!(false);
            assert!(builtin::Runtime::compile(&i).is_err(), "{}", c.id);
        }
        for key in c.configured_settings.keys().chain(c.public_settings.keys()) {
            i.definition.as_mut().unwrap()["settings"] =
                credentials(&c, "fixture-user", "fixture-password");
            i.definition.as_mut().unwrap()["settings"][key] = json!(false);
            assert!(
                builtin::Runtime::compile(&i).is_err(),
                "{}: {key} type",
                c.id
            );
        }
    }
}

#[tokio::test]
async fn every_form_provider_passes_login_cache_download_expiry_edit_and_restart() {
    use futures_util::{stream, StreamExt};
    // Every case runs to completion; failures are collected so one run lists them all.
    let failed: Vec<String> = stream::iter(cases())
        .map(|case| async move {
            let name = case.id.clone();
            let run = tokio::spawn(tokio::time::timeout(
                lifecycle_budget(&case),
                lifecycle(case),
            ));
            match run.await {
                Ok(Ok(())) => None,
                Ok(Err(_)) => Some(format!("{name} (lifecycle deadline)")),
                Err(_) => Some(name),
            }
        })
        // Pinned delays of up to 5 s make some lifecycles minutes long; more parallel
        // cases on a shared host push their searches past the request deadline.
        .buffer_unordered(4)
        .filter_map(|r| async move { r })
        .collect()
        .await;
    assert!(failed.is_empty(), "Failed lifecycles: {failed:?}");
}
async fn lifecycle(c: Case) {
    use axum::{
        body::Bytes,
        extract::State,
        http::{HeaderMap, Method, StatusCode, Uri},
        response::{Html, IntoResponse},
        routing::any,
        Router,
    };
    use rackett::cardigann_exec::request::Charset;
    use rackett::{
        engine::{Download, Engine},
        models::Indexer,
    };
    use std::sync::{Arc, Mutex};
    // Non-UTF-8 sources log in with text native to their code page, proving that the
    // credential body is encoded with the definition charset rather than UTF-8.
    let charset = Plan::compile(&root(&c)).unwrap().charset();
    let user: &'static str = match charset {
        Charset::Utf8 => "fixture-user",
        Charset::Latin1 | Charset::Windows1252 => "fixture-usér",
        Charset::Windows1251 => "fixture-юзер",
        Charset::Windows1250 | Charset::Iso8859_2 => "fixture-użytkownik",
        Charset::Windows1256 => "fixture-مستخدم",
    };
    if charset != Charset::Utf8 {
        assert!(
            c.expected_title.as_deref().is_some_and(|t| !t.is_ascii()),
            "{}: a {} fixture needs a native non-ASCII expected_title",
            c.id,
            charset.label()
        );
    }
    const PASS: &str = "fixture-password";
    const CHANGED: &str = "fixture-changed-password";
    const TORRENT: &[u8] = b"d4:infod4:name7:fixture6:lengthi42eee";
    const HASH: &str = "0123456789abcdef0123456789abcdef01234567";
    struct StateData {
        mode: u8,
        token: String,
        password: String,
        posts: usize,
        hits: Vec<(String, String)>,
        posted: Vec<BTreeMap<String, String>>,
        rotate: bool,
    }
    struct Fixture {
        case: Case,
        body: String,
        base: String,
        charset: Charset,
        user: &'static str,
        state: Mutex<StateData>,
    }
    impl Fixture {
        /// Serves HTML in the definition charset. Non-UTF-8 pages deliberately claim
        /// UTF-8 in the header: the pinned WebResult uses the definition encoding.
        fn html(&self, page: impl Into<String>) -> axum::response::Response {
            let page = page.into();
            if self.charset == Charset::Utf8 {
                return Html(page).into_response();
            }
            let bytes = self.charset.encode(&page).unwrap_or_else(|| {
                panic!(
                    "{}: fixture text outside {}",
                    self.case.id,
                    self.charset.label()
                )
            });
            ([("content-type", "text/html; charset=utf-8")], bytes).into_response()
        }
        /// Parses a form body, decoding percent-escaped bytes with the definition charset.
        fn form(&self, body: &[u8]) -> BTreeMap<String, String> {
            form_pairs(&self.case, self.charset, body)
                .into_iter()
                .collect()
        }
        fn take(&self) -> Vec<(String, String)> {
            std::mem::take(&mut self.state.lock().unwrap().hits)
        }
        fn posts(&self) -> usize {
            self.state.lock().unwrap().posts
        }
    }
    async fn respond(
        State(f): State<Arc<Fixture>>,
        method: Method,
        uri: Uri,
        h: HeaderMap,
        body: Bytes,
    ) -> axum::response::Response {
        // The engine addresses the fixture as 127.0.0.1. Local port scanners on a shared
        // machine (seen as `Host: localhost:<port>`, `user-agent: node`) are not
        // engine requests and must not count as hits.
        if !h
            .get("host")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|host| host.starts_with("127.0.0.1:"))
        {
            return StatusCode::NOT_FOUND.into_response();
        }
        let cookie = h.get("cookie").and_then(|v| v.to_str().ok()).unwrap_or("");
        let cookie = match f.case.literal_cookie.as_deref() {
            None => cookie,
            Some(literal) => cookie
                .strip_prefix(literal)
                .and_then(|rest| rest.strip_prefix("; ").or(rest.is_empty().then_some("")))
                .unwrap_or_else(|| panic!("{}: literal cookie missing: {cookie}", f.case.id)),
        };
        let mut state = f.state.lock().unwrap();
        state.hits.push((uri.to_string(), cookie.into()));
        let form_url = url::Url::parse(&f.base)
            .unwrap()
            .join(&f.case.form_path)
            .unwrap();
        if !f.case.direct_post
            && method == Method::GET
            && uri.to_string() == f.case.form_path
            && (f.case.form_path != f.case.login_path
                || h.get("referer").and_then(|v| v.to_str().ok()) == Some(f.base.as_str()))
        {
            assert_eq!(
                uri.to_string(),
                f.case.form_path,
                "{}: login target",
                f.case.id
            );
            let attributes = f.case.form_attributes.clone().unwrap_or_else(|| {
                format!(
                    "id='{}' action='{}'",
                    f.case.form_id,
                    if f.case.id == "totheglory" {
                        "takelogin.php"
                    } else {
                        &f.case.submit_path
                    }
                )
            });
            if let Some(seed) = &f.case.landing_cookie {
                assert!(
                    cookie.split("; ").any(|p| p == seed),
                    "{}: login cookie seed missing",
                    f.case.id
                );
            }
            let html=format!("<form {attributes}><input type=hidden name=csrf value='fixture-csrf'><input name=username value=obsolete><input type=checkbox name=ignored value=no><input type=hidden name=hidden value=kept></form>{}", f.case.landing_html.as_deref().unwrap_or(""));
            let mut r = f.html(html);
            r.headers_mut()
                .insert("set-cookie", "csrf=fixture-csrf; Path=/".parse().unwrap());
            return r;
        }
        // A `before` request that targets the release's own details page (tmghub posts
        // the thank-you comment to `{{ .DownloadUri }}`) shares its path with searches,
        // the login test and the login submit; only the other POST is the before request.
        if f.case.before_path.as_deref() == Some(uri.path())
            && (f.case.search_path != uri.path()
                || method == Method::POST && uri.to_string() != f.case.submit_path)
        {
            if method == Method::POST {
                let posted = f.form(&body);
                for (k, v) in &f.case.before_fields {
                    assert_eq!(posted.get(k), Some(v), "{}: before field {k}", f.case.id);
                }
            }
            return f.html("<p>Thanks recorded</p>");
        }
        let search_post =
            f.case.search_post && method == Method::POST && uri.path() == f.case.search_path;
        if search_post {
            // Record the body as query pairs so search matching sees one request shape.
            // Legacy-encoded bodies are decoded with the definition charset first.
            let pairs = query_text(&form_pairs(&f.case, f.charset, &body));
            let hit = state.hits.last_mut().unwrap();
            hit.0.push(if hit.0.contains('?') { '&' } else { '?' });
            hit.0.push_str(&pairs);
        }
        if method == Method::POST && !search_post {
            assert_eq!(
                uri.to_string(),
                f.case.submit_path,
                "{}: submit path",
                f.case.id
            );
            assert_eq!(
                h["referer"],
                if f.case.direct_post {
                    f.base.as_str()
                } else {
                    form_url.as_str()
                },
                "{}: referer",
                f.case.id
            );
            assert_eq!(h["content-type"], "application/x-www-form-urlencoded");
            let posted = f.form(&body);
            if f.case.direct_post {
                assert!(
                    cookie.is_empty(),
                    "{}: stale cookie in credential POST",
                    f.case.id
                );
                assert_eq!(
                    posted.len(),
                    f.case.expected_inputs.len(),
                    "{}: unexpected POST fields",
                    f.case.id
                );
            } else {
                assert!(
                    cookie.contains("csrf=fixture-csrf"),
                    "{}: CSRF cookie",
                    f.case.id
                );
                assert_eq!(posted.get("csrf").map(String::as_str), Some("fixture-csrf"));
                assert_eq!(posted.get("hidden").map(String::as_str), Some("kept"));
                assert!(!posted.contains_key("ignored"));
            }
            let expected: BTreeMap<_, _> = f
                .case
                .expected_inputs
                .iter()
                .map(|(k, v)| {
                    let v = match v.as_str() {
                        "{{ .Config.username }}" => f.user.into(),
                        other if other == format!("{{{{ .Config.{} }}}}", f.case.user_key()) => {
                            f.user.into()
                        }
                        "{{ .Config.password }}" => state.password.clone(),
                        "{{ .Config.sitelink }}" => f.base.clone(),
                        "{{ .Config.passid }}" => "0".into(),
                        "{{ .Config.passan }}" => String::new(),
                        "{{ .Config.2facode }}" => String::new(),
                        other if other.starts_with("{{ .Config.") => {
                            let key = other
                                .strip_prefix("{{ .Config.")
                                .unwrap()
                                .strip_suffix(" }}")
                                .unwrap();
                            f.case
                                .configured_settings
                                .get(key)
                                .or_else(|| f.case.public_settings.get(key))
                                .cloned()
                                .unwrap_or_default()
                        }
                        other => other.into(),
                    };
                    (k.clone(), v)
                })
                .collect();
            let valid = expected.iter().all(|(k, v)| posted.get(k) == Some(v));
            state.posted.push(posted);
            state.posts += 1;
            if !valid || state.mode == 5 {
                return f.html(f.case.error_html.clone());
            }
            state.token = format!("session=fixture-form-{}", state.posts);
            let mut r = f.html("<p>Submitted</p>");
            r.headers_mut().insert(
                "set-cookie",
                format!("{}; Path=/; HttpOnly", state.token)
                    .parse()
                    .unwrap(),
            );
            return r;
        }
        assert!(
            method == Method::GET || search_post,
            "{}: method",
            f.case.id
        );
        if state.token.is_empty() || !cookie.split("; ").any(|p| p == state.token) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        if let Some(detail) = &f.case.detail {
            if uri.to_string() == f.case.download_path {
                return f.html(detail.html.clone());
            }
            if uri.to_string() == detail.resolved {
                return TORRENT.into_response();
            }
        } else if uri.to_string() == f.case.download_path {
            return TORRENT.into_response();
        }
        let search = uri.path() == f.case.search_path
            && (search_post
                || f.case.keyword.is_empty()
                || uri.query().is_some_and(|q| {
                    url::form_urlencoded::parse(q.as_bytes()).any(|(key, _)| key == f.case.keyword)
                }));
        if !search {
            assert_eq!(
                uri.to_string(),
                f.case.login_path,
                "{}: authentication test",
                f.case.id
            );
        }
        if search {
            if state.mode == 2 {
                return f.html("<form>Sign in fixture-private-error</form>");
            }
            if state.mode == 4 {
                return (StatusCode::SERVICE_UNAVAILABLE, "fixture-private-error").into_response();
            }
            if state.mode == 6 {
                // Expire exactly once on a search page of a validated session.
                state.mode = 0;
                state.token.clear();
                let mut r = StatusCode::FOUND.into_response();
                r.headers_mut()
                    .insert("location", form_url.as_str().parse().unwrap());
                return r;
            }
        }
        let page = if search && state.mode == 1 {
            empty_for(&f.case, &f.body)
        } else {
            f.body.clone()
        };
        let page = if search && state.mode == 7 {
            page + f.case.search_error_html.as_deref().unwrap()
        } else {
            page
        };
        let mut r = if search && f.case.json && [0, 1].contains(&state.mode) {
            ([("content-type", "application/json")], page).into_response()
        } else {
            f.html(page)
        };
        if search && state.mode == 3 {
            r.headers_mut()
                .insert("cf-mitigated", "challenge".parse().unwrap());
        }
        if search && state.mode == 0 && state.rotate {
            state.token.push_str("-renewed");
            state.rotate = false;
            r.headers_mut().insert(
                "set-cookie",
                format!("{}; Path=/", state.token).parse().unwrap(),
            );
        }
        r
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let fixture = Arc::new(Fixture {
        case: c.clone(),
        body: page(&c, true),
        base: base.clone(),
        charset,
        user,
        state: Mutex::new(StateData {
            mode: 0,
            token: String::new(),
            password: PASS.into(),
            posts: 0,
            hits: vec![],
            posted: vec![],
            rotate: true,
        }),
    });
    let app = Router::new()
        .fallback(any(respond))
        .with_state(fixture.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let engine = Arc::new(Engine::new(path.clone()).await.unwrap());
    // Pinned request delays of 2-3 s make a first authenticated search five spaced
    // requests. The default 20 s deadline is not under test here and is too tight
    // when eight lifecycles share a loaded host, so give each case at least 60 s.
    // A case's timeout_seconds records the minimum its source needs in production.
    engine
        .update_settings(
            json!({"request_timeout_seconds": c.timeout_seconds.unwrap_or(60).max(60)}),
        )
        .await
        .unwrap();
    let i = engine
        .put_indexer(
            Indexer {
                kind: "cardigann".into(),
                name: c.id.clone(),
                url: base.clone(),
                request_delay_ms: 0,
                definition: Some(json!({"builtin_id":c.id,"settings":credentials(&c, user, PASS)})),
                ..Default::default()
            },
            false,
        )
        .await
        .unwrap_or_else(|e| panic!("{} create: {e}", c.id));
    let early = fixture.take();
    assert!(
        early.is_empty(),
        "{}: requests before search {early:?}",
        c.id
    );
    let public = builtin::public_definition(&i).unwrap().to_string();
    assert!(!public.contains(user) && !public.contains(PASS));
    for value in c.configured_settings.values().filter(|v| !v.is_empty()) {
        assert!(
            !public.contains(value),
            "{}: extra credential exposed",
            c.id
        );
    }
    let q = query(&c);
    let first = engine.search(vec![i.id.clone()], q.clone()).await;
    assert!(first.errors.is_empty(), "{}: {:?}", c.id, first.errors);
    assert_eq!(first.results.len(), 1, "{}", c.id);
    assert_eq!(fixture.posts(), 1, "{}", c.id);
    let r = &first.results[0];
    assert_eq!(
        r.title,
        c.expected_title.as_deref().unwrap_or("Fixture Release")
    );
    assert_eq!(r.size, Some(c.expected_size.unwrap_or(1 << 30)));
    assert_eq!(
        (r.seeders, r.peers),
        (
            Some(c.expected_seeders.unwrap_or(11)),
            Some(c.expected_peers.unwrap_or(15))
        )
    );
    assert!(r.categories.contains(&c.category));
    let hits = fixture.take();
    assert_eq!(
        hits[0].0,
        if c.direct_post {
            &c.submit_path
        } else {
            &c.form_path
        }
        .as_str()
    );
    assert!(hits.iter().any(|(u, _)| u == &c.submit_path));
    let is_search = |(u, _): &(String, String)| {
        let url = url::Url::parse(&base).unwrap().join(u).unwrap();
        url.path() == c.search_path
            && (c.keyword.is_empty() || url.query_pairs().any(|(key, _)| key == c.keyword))
    };
    let searches: Vec<_> = hits.iter().filter(|h| is_search(h)).collect();
    assert_eq!(searches.len(), c.request_count);
    // Login and TestLogin run once; the validated session then sends one request per
    // page, as pinned PerformQuery does (docs/session-validation.md).
    let first_search = hits.iter().position(is_search).unwrap();
    assert_eq!(
        hits.len(),
        first_search + c.request_count,
        "{}: request between search pages: {hits:?}",
        c.id
    );
    if !c.no_test {
        assert_eq!(
            hits[first_search - 1].0,
            c.login_path,
            "{}: TestLogin",
            c.id
        );
    }
    // Relogin after a search shows expiry: landing (unless a credential POST),
    // submit and TestLogin (pinned Relogin, `CardigannIndexer.cs:874-890`).
    let relogin = usize::from(!c.direct_post) + 1 + usize::from(!c.no_test);
    if c.no_test {
        // Landing (unless a credential POST), submit and searches; no validation.
        assert_eq!(
            hits.len(),
            usize::from(!c.direct_post) + 1 + c.request_count,
            "{}: {hits:?}",
            c.id
        );
    }
    for (u, _) in searches {
        request_matches(&c, &url::Url::parse(&base).unwrap().join(u).unwrap());
    }
    let cached = engine.search(vec![i.id.clone()], q.clone()).await;
    assert_eq!(cached.cached, 1);
    assert!(fixture.take().is_empty());
    let mut shown = first.clone();
    rackett::protocol::present_results(
        &mut shown,
        "fixture-management-key",
        "http://127.0.0.1:9117",
    );
    // A release whose link is itself a magnet (`download_path: "magnet"`) needs no
    // ticket and sends no download request, as in the public harness.
    let magnet = c.download_path == "magnet";
    if magnet {
        assert!(
            r.link.starts_with("magnet:?xt=urn:btih:") && r.link.contains(HASH),
            "{}: {}",
            c.id,
            r.link
        );
        assert_eq!(shown.results[0].link, r.link, "{}", c.id);
    } else {
        assert!(shown.results[0]
            .link
            .starts_with("http://127.0.0.1:9117/dl/"));
    }
    for secret in [user, PASS, "fixture-form", "fixture-csrf"] {
        assert!(!serde_json::to_string(&shown).unwrap().contains(secret));
    }
    if !magnet {
        assert_eq!(
            engine
                .download(&i.id, r.source_epoch, &r.link)
                .await
                .unwrap_or_else(|e| panic!("{} download: {e}", c.id)),
            match c.detail.as_ref().map(|d| d.resolved.as_str()) {
                Some(magnet) if magnet.starts_with("magnet:") => Download::Magnet(magnet.into()),
                _ => Download::Torrent {
                    bytes: TORRENT.to_vec(),
                    content_type: "application/x-bittorrent".into(),
                },
            }
        );
    }
    let hits = fixture.take();
    assert!(hits.iter().all(|(_, c)| c.contains("-renewed")));
    if let Some(before) = &c.before_path {
        assert!(
            hits.iter()
                .any(|(u, _)| u.split('?').next() == Some(before.as_str())),
            "{}: before request not sent",
            c.id
        );
    }
    if let Some(detail) = &c.detail {
        assert!(hits.iter().any(|(u, _)| *u == c.download_path), "{}", c.id);
        if !detail.resolved.starts_with("magnet:") {
            assert!(hits.iter().any(|(u, _)| *u == detail.resolved), "{}", c.id);
        }
    }
    assert!(engine
        .download(
            &i.id,
            r.source_epoch,
            "https://foreign.invalid/file.torrent"
        )
        .await
        .is_err());
    assert!(fixture.take().is_empty());
    engine
        .patch_indexer(&i.id, json!({"name":"Edited fixture"}))
        .await
        .unwrap();
    if !magnet {
        assert!(engine
            .download(&i.id, r.source_epoch, &r.link)
            .await
            .is_err());
    }
    assert!(fixture.take().is_empty());
    drop(engine);
    let engine = Arc::new(Engine::new(path.clone()).await.unwrap());
    let restarted = engine.search(vec![i.id.clone()], q.clone()).await;
    assert!(
        restarted.errors.is_empty(),
        "{}: {:?}",
        c.id,
        restarted.errors
    );
    assert_eq!(restarted.results.len(), 1);
    assert_eq!(fixture.posts(), 1);
    let hits = fixture.take();
    assert!(hits.iter().all(|(_, cookie)| cookie.contains("-renewed")));
    // Validation is in memory: the restored session is tested once, then trusted.
    assert_eq!(
        hits.len(),
        usize::from(!c.no_test) + c.request_count,
        "{}: restart: {hits:?}",
        c.id
    );
    if !c.no_test {
        assert_eq!(hits[0].0, c.login_path, "{}: restart TestLogin", c.id);
    }
    // Configured credential changes retire the old session and ticket, not just the cache.
    // A credential-less source has nothing to change.
    if !c.no_credentials {
        fixture.state.lock().unwrap().password = CHANGED.into();
        engine
            .patch_indexer(
                &i.id,
                json!({"definition":{"builtin_id":c.id,"settings":{"password":CHANGED}}}),
            )
            .await
            .unwrap();
        assert!(engine
            .download(
                &i.id,
                restarted.results[0].source_epoch,
                &restarted.results[0].link
            )
            .await
            .is_err());
        let changed = engine.search(vec![i.id.clone()], q.clone()).await;
        assert!(changed.errors.is_empty(), "{}: {:?}", c.id, changed.errors);
        assert_eq!(fixture.posts(), 2);
        let password_input = c
            .expected_inputs
            .iter()
            .find(|(_, v)| v.as_str() == "{{ .Config.password }}")
            .unwrap()
            .0;
        assert_eq!(
            fixture.state.lock().unwrap().posted[1][password_input],
            CHANGED
        );
        let hits = fixture.take();
        // The re-login starts from scratch: no cookie, or only the declared `login.cookies`
        // seed, which pinned DoLogin sets as the cookie header (`CardigannIndexer.cs:1026`).
        assert!(
            hits[0].1.is_empty() || Some(hits[0].1.as_str()) == c.landing_cookie.as_deref(),
            "{}: old session reused after password change",
            c.id
        );
        assert_eq!(
            hits.len(),
            relogin + c.request_count,
            "{}: changed credentials: {hits:?}",
            c.id
        );
    }
    engine.clear_cache().await;
    fixture.state.lock().unwrap().mode = 6;
    let renewed = engine.search(vec![i.id.clone()], q.clone()).await;
    assert!(renewed.errors.is_empty(), "{}: {:?}", c.id, renewed.errors);
    assert_eq!(fixture.posts(), 2 + usize::from(!c.no_credentials));
    // The expired page, one relogin, its retry, then the remaining pages.
    let hits = fixture.take();
    assert!(is_search(&hits[0]), "{}: {hits:?}", c.id);
    assert_eq!(
        hits.len(),
        1 + relogin + c.request_count,
        "{}: expiry relogin: {hits:?}",
        c.id
    );
    let mut modes = vec![1, 2, 3, 4];
    if c.search_error_html.is_some() {
        modes.push(7);
    }
    for mode in modes {
        engine.clear_cache().await;
        fixture.state.lock().unwrap().mode = mode;
        let before = fixture.posts();
        let result = engine.search(vec![i.id.clone()], q.clone()).await;
        assert!(result.results.is_empty(), "{} mode {mode}", c.id);
        // Without a test a marker-less HTML page is an empty result and never a
        // relogin (pinned CheckIfLoginIsNeeded, `CardigannIndexer.cs:977-980`).
        assert_eq!(
            result.errors.is_empty(),
            mode == 1 || (mode == 2 && c.no_test && !c.json),
            "{} mode {mode}: {:?}",
            c.id,
            result.errors
        );
        assert_eq!(
            fixture.posts() - before,
            usize::from(mode == 2 && !c.no_test),
            "{} mode {mode}: bounded auth retry: {:?}",
            c.id,
            result.errors
        );
        let text = serde_json::to_string(&result).unwrap();
        for secret in [user, PASS, CHANGED, "fixture-private-error"] {
            assert!(!text.contains(secret));
        }
        let hits = fixture.take();
        assert!(!hits.is_empty());
        if !c.no_test {
            // Mode 1 pages trust the validated session and mode 2 relogs on its
            // first page. Its failed retry clears that trust, so later searches
            // run TestLogin first.
            match mode {
                1 => assert_eq!(hits.len(), c.request_count, "{} mode 1", c.id),
                2 => assert!(is_search(&hits[0]), "{} mode 2: {hits:?}", c.id),
                _ => assert!(
                    hits[0].0 == c.login_path && hits.len() > 1 && is_search(&hits[1]),
                    "{} mode {mode}: {hits:?}",
                    c.id
                ),
            }
        }
    }
    fixture.state.lock().unwrap().mode = 0;
    let recovered = engine.search(vec![i.id.clone()], q.clone()).await;
    assert!(
        recovered.errors.is_empty(),
        "{}: {:?}",
        c.id,
        recovered.errors
    );
    assert_eq!(recovered.results.len(), 1);
    // Mode 7 pages pass the login check, so only a mode 4 failure leaves TestLogin.
    let hits = fixture.take();
    assert_eq!(
        hits.len(),
        usize::from(!c.no_test && c.search_error_html.is_none()) + c.request_count,
        "{}: recovered: {hits:?}",
        c.id
    );
    // A wrong-password/error page on reauthentication must never reach extraction.
    engine.clear_cache().await;
    {
        let mut s = fixture.state.lock().unwrap();
        s.token.clear();
        s.mode = 5;
    }
    let failed = engine.search(vec![i.id.clone()], q).await;
    assert!(!failed.errors.is_empty());
    assert!(failed.results.is_empty());
    if c.id == "diablotorrent" {
        assert!(failed.errors[0].message.contains("rejected form login"));
    }
    assert!(!serde_json::to_string(&failed)
        .unwrap()
        .contains("fixture-private-error"));
    // The server stays bound until the test binary exits. A dropped engine's background
    // stale refresh can outlive the case; aborting here would free the port for a later
    // case's fixture, which would then receive that stray traffic.
    drop(server);
}

#[test]
fn mvgroup_and_p2pbg_keep_their_distinct_settings_and_row_semantics() {
    for c in cases().into_iter().filter(|c| c.id.starts_with("mvgroup")) {
        let p = Plan::compile(&root(&c)).unwrap();
        let settings = BTreeMap::from([
            ("dropbbc".into(), json!(true)),
            ("stripS01E01".into(), json!(true)),
            ("hidef".into(), json!(true)),
            ("withsubs".into(), json!("eng")),
            ("seed".into(), json!(true)),
        ]);
        let q = SearchQuery {
            params: BTreeMap::from([("q".into(), "Fixture S01E01".into())]),
        };
        let ctx = p.context(&settings, &q, "2026-10-29T12:00:00Z".parse().unwrap());
        let req = p.build_requests(&ctx).unwrap().remove(0);
        let u = url::Url::parse(&req.url).unwrap();
        let pairs: BTreeMap<_, _> = u.query_pairs().collect();
        assert_eq!(pairs["filter"], "hd+Fixture ");
        assert_eq!(pairs["withsubs"], "eng");
        assert_eq!(pairs["seed"], "1");
        let rows = p
            .extract(
                &req,
                &page(&c, false).replace("Fixture Release", "BBC Fixture Release"),
                &ctx,
            )
            .unwrap();
        assert_eq!(rows[0].title, "Fixture Release");
        assert_eq!(rows[0].size, Some(512 * 1024 * 1024));
        assert_eq!(
            rows[0].attributes["magnet"],
            "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"
        );
        // This informational setting is displayed as information, never a credential.
        let catalog = builtin::catalog();
        let setting = catalog.iter().find(|v| v["id"] == c.id).unwrap()["settings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == "info_category_8000")
            .unwrap();
        assert_eq!(setting["type"], "info");
        assert_eq!(setting["secret"], false);
    }
    let c = cases().into_iter().find(|c| c.id == "p2pbg").unwrap();
    let p = Plan::compile(&root(&c)).unwrap();
    let ctx = p.context(
        &BTreeMap::new(),
        &query(&c),
        "2026-10-29T12:00:00Z".parse().unwrap(),
    );
    let req = p.build_requests(&ctx).unwrap().remove(0);
    let body = page(&c, false);
    let row = p.extract(&req, &body, &ctx).unwrap().remove(0);
    assert_eq!(row.attributes["minimumratio"], "1");
    assert_eq!(row.attributes["minimumseedtime"], "259200");
    assert!(!row.attributes.contains_key("magnet"));
    let recommended = body.replace(
        "class=\"torrent-index__table\"",
        "class=\"torrent-index__table torrent-index__table--recommended\"",
    );
    assert!(p.extract(&req, &recommended, &ctx).unwrap().is_empty());
    let relative = body.replace("28/10/2026", "Вчера 10:30");
    assert_eq!(
        p.extract(&req, &relative, &ctx).unwrap()[0]
            .published
            .as_deref(),
        Some("2026-10-28T10:30:00Z")
    );
    assert!(p
        .extract(&req, &body.replace("28/10/2026", "31/02/2026"), &ctx)
        .is_err());
}

#[test]
fn post_layout_reuse_requires_equal_pinned_search_and_caps() {
    fn without_locations(node: &cardigann::Node) -> serde_json::Value {
        fn strip(v: &mut serde_json::Value) {
            match v {
                serde_json::Value::Object(m) => {
                    m.remove("location");
                    for v in m.values_mut() {
                        strip(v);
                    }
                }
                serde_json::Value::Array(a) => {
                    for v in a {
                        strip(v);
                    }
                }
                _ => (),
            }
        }
        let mut value = serde_json::to_value(node).unwrap();
        strip(&mut value);
        value
    }
    for (id, cookie) in [
        ("hdspace", "hdspacecookie"),
        ("pwtorrents", "pwtorrents2fa"),
        ("xwtclassics", "xwtclassics2fa"),
    ] {
        let c = cases().into_iter().find(|c| c.id == id).unwrap();
        assert_eq!(c.search_fixture, cookie);
        let cookie = cardigann::parse(
            builtin::SOURCES
                .iter()
                .find(|(i, _)| *i == cookie)
                .unwrap()
                .1,
            cardigann::Limits::default(),
        )
        .unwrap();
        // Compare parsed mappings, not merely the row-selector string or an ID family.
        for key in ["search", "caps"] {
            assert_eq!(
                without_locations(root(&c).get(key).unwrap()),
                without_locations(cookie.get(key).unwrap()),
                "{id}: {key}"
            );
        }
    }
}

#[test]
fn post_layouts_preserve_alternate_downloads_dates_promotions_and_rows() {
    let all = cases();
    let now = "2026-10-29T12:00:00Z".parse().unwrap();
    for (id, target) in [
        ("pwtorrents", "/seeddownload.php?id=42"),
        ("xwtclassics", "/download2.php?id=42"),
        ("xtremebytes", "/download2.php?id=42"),
    ] {
        let c = all.iter().find(|c| c.id == id).unwrap();
        let p = Plan::compile(&root(c)).unwrap();
        let ctx = p.context(
            &BTreeMap::from([("seedbox".into(), json!(true))]),
            &query(c),
            now,
        );
        let req = p.build_requests(&ctx).unwrap().remove(0);
        let rows = p.extract(&req, &page(c, true), &ctx).unwrap();
        assert!(rows[0].link.ends_with(target), "{id}");
    }
    for (id, markup, down, up) in [
        ("backups", "<img src='pic/silverdownload.gif'>", 0.5, 1.0),
        ("cinemageddon", "<img src='pic/golden1.gif'>", 0.9, 1.1),
        ("carpathians", "<img src='pic/doubledouble2.png'>", 0.0, 4.0),
        (
            "hdroute",
            "<figure class='sprite_dlp070'></figure>",
            0.7,
            1.0,
        ),
        ("nethd", "<span class=label>30%</span>", 0.7, 1.0),
        (
            "pttime",
            "<font class='promotion twouphalfdown'></font>",
            0.5,
            2.0,
        ),
        ("mdan", "<img src='./pic/silver2.gif'>", 0.5, 1.0),
    ] {
        let c = all.iter().find(|c| c.id == id).unwrap();
        let p = Plan::compile(&root(c)).unwrap();
        let ctx = p.context(&BTreeMap::new(), &query(c), now);
        let req = p.build_requests(&ctx).unwrap().remove(0);
        let body = page(c, true).replace(&c.free_markup, markup);
        let rows = p.extract(&req, &body, &ctx).unwrap();
        assert_eq!(
            rows[0].attributes["downloadvolumefactor"]
                .parse::<f64>()
                .unwrap(),
            down,
            "{id}"
        );
        assert_eq!(
            rows[0].attributes["uploadvolumefactor"]
                .parse::<f64>()
                .unwrap(),
            up,
            "{id}"
        );
    }
    for (id, before, after, expected) in [
        (
            "fanoin",
            "Vakar 10:30",
            "Šodien 10:30",
            "2026-10-29T10:30:00Z",
        ),
        (
            "metalguru",
            "Oct 28 2026 10:30 AM",
            "Today 10:30",
            "2026-10-29T10:30:00Z",
        ),
        (
            "mdan",
            "10:30 28-10-2026",
            "Ontem 10:30",
            "2026-10-28T10:30:00Z",
        ),
        (
            "nethd",
            "2026-10-2810:30:00",
            "2 giờ",
            "2026-10-29T10:00:00Z",
        ),
        (
            "pttime",
            "<span title=\"2026-10-28 10:30:00\">yesterday</span>",
            "2026-10-2810:30:00",
            "2026-10-28T02:30:00Z",
        ),
    ] {
        let c = all.iter().find(|c| c.id == id).unwrap();
        let p = Plan::compile(&root(c)).unwrap();
        let ctx = p.context(&BTreeMap::new(), &query(c), now);
        let req = p.build_requests(&ctx).unwrap().remove(0);
        let body = page(c, false);
        assert!(body.contains(before));
        let rows = p.extract(&req, &body.replace(before, after), &ctx).unwrap();
        assert_eq!(rows[0].published.as_deref(), Some(expected), "{id}");
    }
    let c = all.iter().find(|c| c.id == "scenerush").unwrap();
    let p = Plan::compile(&root(c)).unwrap();
    let ctx = p.context(&BTreeMap::new(), &query(c), now);
    let req = p.build_requests(&ctx).unwrap().remove(0);
    let rows = p
        .extract(
            &req,
            &page(c, true).replace(&c.free_markup, "<i class='fg-audiopt' title='PT'></i>"),
            &ctx,
        )
        .unwrap();
    assert_eq!(rows[0].title, "Fixture Release Portuguese PT-PT");
    let c = all.iter().find(|c| c.id == "mdan").unwrap();
    let p = Plan::compile(&root(c)).unwrap();
    let ctx = p.context(&BTreeMap::new(), &query(c), now);
    let req = p.build_requests(&ctx).unwrap().remove(0);
    let body = page(c, true);
    let without_pair = body.replace(
        "<tr id=\"kdescr42\"><td>Fixture description</td><td><img src=\"/fixture.jpg\"></td></tr>",
        "",
    );
    assert_ne!(body, without_pair);
    assert_eq!(
        p.extract(&req, &without_pair, &ctx).unwrap_err().path,
        "$.search.rows.after"
    );
}

#[tokio::test]
async fn pttime_adult_path_passes_the_same_complete_authenticated_lifecycle() {
    let mut c = cases().into_iter().find(|c| c.id == "pttime").unwrap();
    c.category = 6000;
    c.custom_category = Some(100440);
    c.category_input = Some(["cat440".into(), "1".into()]);
    c.search_path = "/adults.php".into();
    c.search_fixture = "post:pttime-adults".into();
    check_layout(c.clone());
    tokio::time::timeout(lifecycle_budget(&c), lifecycle(c))
        .await
        .unwrap();
}

#[test]
fn second_post_batch_preserves_provider_filters_and_declared_limitations() {
    let all = cases();
    let now = "2026-10-29T12:00:00Z".parse().unwrap();
    for (id, markup, down, up) in [
        ("ceskeforum", "<img class=pro_50pctdown2up>", 0.5, 2.0),
        ("ceskeforum", "<img class=pro_30pctdown>", 0.3, 1.0),
        ("hd-club", "<img src='gold/gold75.gif'>", 0.25, 1.0),
        ("hd-club", "<img src='gold/gold50.gif'>", 0.5, 1.0),
        ("hd-club", "<img src='gold/gold25.gif'>", 0.75, 1.0),
        (
            "insanetracker",
            "<a href='?doubleup=three'>Triple</a>",
            1.0,
            3.0,
        ),
        ("mnv", "<img src='pic/oupic.gif'>", 0.0, 1.0),
        ("sportscult", "<img src='gold/silver.gif'>", 0.5, 1.0),
        ("thenewfun", "<img src='pic/freeleech.gif'>", 1.0, 0.0),
    ] {
        let c = all.iter().find(|c| c.id == id).unwrap();
        let p = Plan::compile(&root(c)).unwrap();
        let ctx = p.context(&BTreeMap::new(), &query(c), now);
        let req = p.build_requests(&ctx).unwrap().remove(0);
        let body = page(c, true).replace(&c.free_markup, markup);
        let r = p.extract(&req, &body, &ctx).unwrap().remove(0);
        assert_eq!(
            r.attributes["downloadvolumefactor"].parse::<f64>().unwrap(),
            down,
            "{id}"
        );
        assert_eq!(
            r.attributes["uploadvolumefactor"].parse::<f64>().unwrap(),
            up,
            "{id}"
        );
    }
    let c = all.iter().find(|c| c.id == "bibliotik").unwrap();
    let p = Plan::compile(&root(c)).unwrap();
    let ctx = p.context(&BTreeMap::new(), &query(c), now);
    let req = p.build_requests(&ctx).unwrap().remove(0);
    for author_class in ["authorLink", "editorLink"] {
        let body = page(c, false).replace("<div class=title>", &format!("<span class={author_class}>Fixture Author</span><span class=torYear>2026</span><span class=torFormat>EPUB</span><span class=torRetail>Retail</span><div class=title>"));
        assert_eq!(
            p.extract(&req, &body, &ctx).unwrap()[0].title,
            "Fixture Release by Fixture Author 2026 EPUB Retail"
        );
    }
    let c = all.iter().find(|c| c.id == "exkinoray").unwrap();
    let p = Plan::compile(&root(c)).unwrap();
    let settings = BTreeMap::from([("addrussiantotitle".into(), json!(true))]);
    let ctx = p.context(&settings, &query(c), now);
    let req = p.build_requests(&ctx).unwrap().remove(0);
    let body = page(c, false).replace("Fixture Release", "Fixture Release Сезон 2 WEB DL Rip");
    assert_eq!(
        p.extract(&req, &body, &ctx).unwrap()[0].title,
        "Fixture Release S2 WEB-DL RUS"
    );
    let c = all.iter().find(|c| c.id == "ceskeforum").unwrap();
    let p = Plan::compile(&root(c)).unwrap();
    let ctx = p.context(&BTreeMap::new(), &query(c), now);
    let req = p.build_requests(&ctx).unwrap().remove(0);
    let body = page(c, false).replace(
        "<span title=\"2026-10-28 10:30:00\">elapsed</span>",
        "2026-10-2810:30:00",
    );
    assert_eq!(
        p.extract(&req, &body, &ctx).unwrap()[0]
            .published
            .as_deref(),
        Some("2026-10-28T08:30:00Z")
    );
    // Upstream also trims only at dateparse. Its literal `by` removal can leave
    // two internal spaces before the appended zone, outside this exact layout.
    // Retain a whole-response error, rather than silently inventing a date.
    let c = all.iter().find(|c| c.id == "losslessclub").unwrap();
    let p = Plan::compile(&root(c)).unwrap();
    let ctx = p.context(&BTreeMap::new(), &query(c), now);
    let req = p.build_requests(&ctx).unwrap().remove(0);
    let body = page(c, false).replace("28/10/26", "28/10/26 by ");
    assert_eq!(
        p.extract(&req, &body, &ctx).unwrap_err().path,
        "$.search.fields.date"
    );
}

#[test]
fn puntotorrent_title_suffix_follows_each_quality_branch() {
    // The title is the cell text with `\W` turned into dots, then ` ` plus the
    // quality markers that each optional field adds (docs/nested-has-selectors.md
    // covers the repeated `a:not(...)a:not(...)` compound the BluRay field uses).
    // Expected values are derived by hand from the pinned selectors: BluRay needs
    // `1080` without HDTV/REMUX/WEB, or `2160` together with a lower-case `uhd`.
    let all = cases();
    let c = all.iter().find(|c| c.id == "puntotorrent").unwrap();
    let p = Plan::compile(&root(c)).unwrap();
    let ctx = p.context(
        &BTreeMap::new(),
        &query(c),
        "2026-10-29T12:00:00Z".parse().unwrap(),
    );
    let req = p.build_requests(&ctx).unwrap().remove(0);
    let body = page(c, false);
    const MAIN: &str = "Fixture Release BluRay 1080p 4K";
    assert!(body.contains(MAIN));
    for (text, expected) in [
        (
            MAIN,
            "Fixture.Release.BluRay.1080p.4K BluRay.2160p.Spanish-PuntoTorrent1080p.Spanish-PuntoTorrent",
        ),
        (
            "Fixture Release 2160p uhd",
            "Fixture.Release.2160p.uhd BluRay.2160p.Spanish-PuntoTorrent",
        ),
        (
            "Fixture Release REMUX 1080p",
            "Fixture.Release.REMUX.1080p Remux.1080p.Spanish-PuntoTorrent",
        ),
        (
            "Fixture Release WEB-DL 720p",
            "Fixture.Release.WEB.DL.720p WEBDL.720p.Spanish-PuntoTorrent",
        ),
        (
            "Fixture Release HDTV 720p",
            "Fixture.Release.HDTV.720p HDTV.720p.Spanish-PuntoTorrent",
        ),
        (
            "Fixture Release DVDRip Xvid",
            "Fixture.Release.DVDRip.Xvid DVD.Spanish-PuntoTorrent480p.Spanish-PuntoTorrent",
        ),
        (
            "Fixture Release VOSE 720p",
            "Fixture Release VOSE 720p | English 720p.Spanish-PuntoTorrent",
        ),
    ] {
        let rows = p
            .extract(&req, &body.replace(MAIN, text), &ctx)
            .unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(rows[0].title, expected, "{text}");
    }
}
