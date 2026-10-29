//! JSON `response.noResultsMessage: ""` per search path. Source: Jackett fe8876d2
//! `CardigannIndexer.cs:1608-1645` (redirect/re-login, exact HTTP 200, then the empty-string
//! check before `JToken.Parse`), `:951-975` (`CheckIfLoginIsNeeded`) and
//! `WebResult.cs:55-74` (`ContentString` keeps a decoded BOM; `IsRedirect` includes Refresh).
//! Synthetic fixtures only; no live responses or credentials.
use chrono::{TimeZone, Utc};
use rackett::{
    cardigann::{parse, Limits},
    cardigann_exec::{
        request::BuiltRequest, template::Context, ExecutionError, Plan, ResponseMeta,
    },
    models::{Release, SearchQuery},
};
use std::collections::BTreeMap;

fn pinned(name: &str) -> String {
    std::fs::read_to_string(format!("vendor/jackett/definitions/{name}.yml")).unwrap()
}
fn compile(source: &str) -> Result<Plan, ExecutionError> {
    Plan::compile(&parse(source, Limits::default()).unwrap())
}
/// Parsing must succeed so that compilation, not the parser, decides the outcome.
fn compile_parsed(source: &str) -> Result<Plan, ExecutionError> {
    Plan::compile(&parse(source, Limits::default()).expect("parser retains repeated keys"))
}
const FIELDS: &str = "  rows: {selector: items}\n  fields:\n    category: {text: 1}\n    seeders: {text: 1}\n    size: {text: 1024}\n    title: {selector: name}\n    download: {selector: link}\n";
/// Synthetic public definition with the given `search.paths` entries.
fn definition(paths: &str) -> String {
    format!(
        "id: fixture\nname: Fixture\ndescription: Synthetic fixture\nlanguage: en-US\ntype: public\nencoding: UTF-8\nlinks: [https://example.test/]\ncaps:\n  categories: {{1: Other}}\n  modes: {{search: [q]}}\nsettings: []\nsearch:\n  paths:\n{paths}{FIELDS}"
    )
}
const POLICY: &str =
    "    - path: api\n      response:\n        type: json\n        noResultsMessage: \"\"\n";
const PLAIN: &str = "    - path: plain\n      response:\n        type: json\n";
const ROWS: &str = r#"{"items":[{"name":"Fixture release","link":"magnet:?xt=urn:btih:0123456789012345678901234567890123456789"}]}"#;
const OK: ResponseMeta = ResponseMeta {
    status: 200,
    redirected: false,
    refresh: false,
    body_bytes: 0,
};
fn meta(body: &str) -> ResponseMeta {
    ResponseMeta {
        body_bytes: body.len(),
        ..OK
    }
}
fn requests(plan: &Plan) -> (Vec<BuiltRequest>, Context) {
    let query = SearchQuery {
        params: BTreeMap::from([("q".into(), "ubuntu".into()), ("t".into(), "search".into())]),
    };
    let context = plan.context(
        &BTreeMap::new(),
        &query,
        Utc.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap(),
    );
    (plan.build_requests(&context).unwrap(), context)
}
fn response_error(result: Result<Vec<Release>, ExecutionError>, message: &str) {
    let e = result.unwrap_err();
    assert_eq!(e.path, "$.response", "{e}");
    assert_eq!(e.message, message, "{e}");
}
const NEEDS_META: &str = "Response metadata is required for the no-results policy";
const NOT_200: &str = "No-results policy requires HTTP 200";
const REDIRECTED: &str = "No-results policy rejects redirected responses";
const INVALID_JSON: &str = "Invalid or over-deep JSON";

#[test]
fn pinned_empty_message_definitions_compile_with_login_blockers() {
    for name in ["czteam-api", "digitalcore-api", "superbits"] {
        let plan = compile(&pinned(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(
            plan.blockers().iter().any(|b| b.path == "$.login"),
            "{name}"
        );
        let (built, _) = requests(&plan);
        assert!(!built.is_empty(), "{name}");
        assert!(built.iter().all(BuiltRequest::empty_no_results), "{name}");
    }
    // A different no-results semantic stays blocked.
    let e = compile(&pinned("hdforever-api")).err().unwrap();
    assert_eq!(e.path, "$.search.rows.missingAttributeEqualsNoResults");
}

#[test]
fn pinned_definition_extracts_rows_and_empty_bodies() {
    let plan = compile(&pinned("czteam-api")).unwrap();
    let (built, ctx) = requests(&plan);
    let body = r#"[{"name":"Fixture release","url":"https://example.test/d/1","download_link":"https://example.test/t/1","category_id":1,"seeders":1,"leechers":0,"times_completed":0,"files":1,"size":1024,"upload_date":"2026-05-09 18:57:50","freeleech":0,"doubleup":0,"internal":0}]"#;
    let rows = plan
        .extract_response(&built[0], &meta(body), body, &ctx)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "Fixture release");
    assert!(plan
        .extract_response(&built[0], &OK, "", &ctx)
        .unwrap()
        .is_empty());
    response_error(plan.extract(&built[0], "", &ctx), NEEDS_META);
}

#[test]
fn compile_accepts_only_an_exact_quoted_empty_string_on_json_paths() {
    let with = |value: &str| {
        definition(&format!(
            "    - path: api\n      response:\n        type: json\n        noResultsMessage: {value}\n"
        ))
    };
    for value in ["\"\"", "''"] {
        let plan = compile(&with(value)).unwrap();
        assert!(requests(&plan).0[0].empty_no_results(), "{value}");
    }
    for value in [
        "none",
        "\"none\"",
        "\" \"",
        "\"\\t\"",
        "",
        "~",
        "null",
        "\"null\"",
        "[]",
        "[\"\"]",
        "{}",
        "|\n          x",
        ">\n          x",
    ] {
        let e = compile(&with(value))
            .err()
            .unwrap_or_else(|| panic!("{value}"));
        assert!(e.to_string().contains("noResultsMessage"), "{value}: {e}");
    }
    // Repeated keys inside the response block are ambiguous; the parser retains them.
    for response in [
        "        type: json\n        noResultsMessage: \"\"\n        noResultsMessage: \"\"\n",
        "        type: json\n        noResultsMessage: \"\"\n        noResultsMessage: none\n",
        "        type: json\n        type: json\n        noResultsMessage: \"\"\n",
        "        noResultsMessage: \"\"\n        type: json\n        type: xml\n",
    ] {
        let source = definition(&format!("    - path: api\n      response:\n{response}"));
        let e = compile_parsed(&source).err().unwrap();
        assert_eq!(e.path, "$.search", "{response}");
        assert_eq!(
            e.message,
            "$.search.paths[0].response: repeated structural property with noResultsMessage"
        );
    }
    // Non-JSON paths reject rather than ignore the setting.
    for response in [
        "        type: html\n        noResultsMessage: \"\"\n",
        "        type: xml\n        noResultsMessage: \"\"\n",
        "        noResultsMessage: \"\"\n",
    ] {
        let source = definition(&format!("    - path: api\n      response:\n{response}"));
        assert!(compile(&source).is_err(), "{response}");
    }
    // Other response keys remain blocked next to the policy.
    let source = definition(
        "    - path: api\n      response:\n        type: json\n        noResultsMessage: \"\"\n        other: x\n",
    );
    assert!(compile(&source)
        .err()
        .unwrap()
        .to_string()
        .contains("other"));
    // A single-request search has no response block.
    let source = definition("    - path: plain\n").replace(
        "  paths:\n    - path: plain\n",
        "  path: api\n  response:\n    type: json\n    noResultsMessage: \"\"\n",
    );
    assert!(compile(&source).is_err());
}

#[test]
fn mixed_plans_reject_an_html_path_with_the_policy() {
    let html =
        "    - path: page\n      response:\n        type: html\n        noResultsMessage: \"\"\n";
    let e = compile(&definition(&format!("{POLICY}{html}")))
        .err()
        .unwrap();
    assert!(e.to_string().contains("paths[1].response"), "{e}");
}

#[test]
fn the_policy_is_per_path_and_not_inherited() {
    let plan = compile(&definition(&format!("{POLICY}{PLAIN}{POLICY}").replacen(
        "path: api\n",
        "path: first\n",
        1,
    )))
    .unwrap();
    let (built, ctx) = requests(&plan);
    let flags: Vec<_> = built.iter().map(BuiltRequest::empty_no_results).collect();
    assert_eq!(flags, [true, false, true]);
    // The plain path keeps ordinary JSON behavior even with trusted metadata.
    response_error(
        plan.extract_response(&built[1], &OK, "", &ctx),
        INVALID_JSON,
    );
    assert!(plan
        .extract_response(&built[0], &OK, "", &ctx)
        .unwrap()
        .is_empty());
    let plain = compile(&definition(PLAIN)).unwrap();
    let (built, _) = requests(&plain);
    assert!(!built[0].empty_no_results());
}

#[test]
fn metadata_less_extraction_fails_closed_for_policy_requests() {
    let plan = compile(&definition(POLICY)).unwrap();
    let (built, ctx) = requests(&plan);
    for body in ["", ROWS, "{}", "not JSON"] {
        response_error(plan.extract(&built[0], body, &ctx), NEEDS_META);
    }
    // Ordinary requests are unchanged.
    let plain = compile(&definition(PLAIN)).unwrap();
    let (built, ctx) = requests(&plain);
    assert_eq!(plain.extract(&built[0], ROWS, &ctx).unwrap().len(), 1);
    response_error(plain.extract(&built[0], "", &ctx), INVALID_JSON);
}

#[test]
fn trusted_metadata_gates_every_policy_response() {
    let plan = compile(&definition(POLICY)).unwrap();
    let (built, ctx) = requests(&plan);
    let request = &built[0];
    assert!(plan
        .extract_response(request, &OK, "", &ctx)
        .unwrap()
        .is_empty());
    let rows = plan
        .extract_response(request, &meta(ROWS), ROWS, &ctx)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "Fixture release");
    let empty = r#"{"items":[]}"#;
    assert!(plan
        .extract_response(request, &meta(empty), empty, &ctx)
        .unwrap()
        .is_empty());
    // Nonempty JSON keeps ordinary row errors.
    assert_eq!(
        plan.extract_response(request, &meta("{}"), "{}", &ctx)
            .unwrap_err()
            .path,
        "$.search.rows"
    );
    // Only zero raw bytes are string.Empty: a BOM, whitespace or malformed text parse as usual.
    for body in ["\u{feff}", " ", "\n", "\r\n", "\0", "null x", "[", "\"\""] {
        assert!(
            plan.extract_response(request, &meta(body), body, &ctx)
                .is_err(),
            "{body:?}"
        );
    }
    response_error(
        plan.extract_response(request, &meta("\u{feff}"), "\u{feff}", &ctx),
        INVALID_JSON,
    );
    // Raw bytes and decoded text must both be empty.
    let bytes = ResponseMeta {
        body_bytes: 3,
        ..OK
    };
    response_error(
        plan.extract_response(request, &bytes, "", &ctx),
        INVALID_JSON,
    );
    // Exactly HTTP 200, for empty and nonempty bodies alike.
    for status in [
        100, 201, 202, 204, 206, 301, 302, 304, 400, 401, 403, 404, 429, 500, 503,
    ] {
        for body in ["", ROWS] {
            let m = ResponseMeta {
                status,
                ..meta(body)
            };
            response_error(plan.extract_response(request, &m, body, &ctx), NOT_200);
        }
    }
    // Any followed redirect or Refresh header (upstream re-login) rejects every response.
    for m in [
        ResponseMeta {
            redirected: true,
            ..OK
        },
        ResponseMeta {
            refresh: true,
            ..OK
        },
    ] {
        for body in ["", ROWS] {
            let m = ResponseMeta {
                body_bytes: body.len(),
                ..m
            };
            response_error(plan.extract_response(request, &m, body, &ctx), REDIRECTED);
        }
    }
}

#[test]
fn errors_are_static_and_never_include_the_response() {
    let plan = compile(&definition(POLICY)).unwrap();
    let (built, ctx) = requests(&plan);
    let secret = r#"{"error":"passkey synthetic-secret-1234 invalid"}"#;
    for m in [
        ResponseMeta {
            status: 403,
            ..meta(secret)
        },
        ResponseMeta {
            redirected: true,
            ..meta(secret)
        },
    ] {
        let e = plan
            .extract_response(&built[0], &m, secret, &ctx)
            .unwrap_err();
        assert!(!e.to_string().contains("synthetic-secret"), "{e}");
        assert!(!e.to_string().contains("example.test"), "{e}");
    }
}

#[test]
fn policy_flag_mismatches_fail_closed() {
    let plan = compile(&definition(POLICY)).unwrap();
    let (built, ctx) = requests(&plan);
    // A policy request rewritten to another response type.
    let mut html = built[0].clone();
    html.response_type = "html".into();
    response_error(
        plan.extract_response(&html, &OK, "", &ctx),
        "No-results policy does not match this plan",
    );
    // A policy request handed to a plan without the policy.
    let plain = compile(&definition(PLAIN)).unwrap();
    let (_, plain_ctx) = requests(&plain);
    response_error(
        plain.extract_response(&built[0], &OK, "", &plain_ctx),
        "No-results policy does not match this plan",
    );
    // An ordinary request never gains the policy from metadata alone.
    let (plain_built, _) = requests(&plain);
    response_error(
        plan.extract_response(&plain_built[0], &OK, "", &ctx),
        INVALID_JSON,
    );
}

#[test]
fn ordinary_json_responses_ignore_metadata() {
    let plan = compile(&definition(PLAIN)).unwrap();
    let (built, ctx) = requests(&plan);
    for status in [200, 201, 204, 206] {
        for redirected in [false, true] {
            let m = ResponseMeta {
                status,
                redirected,
                ..meta(ROWS)
            };
            assert_eq!(
                plan.extract_response(&built[0], &m, ROWS, &ctx)
                    .unwrap()
                    .len(),
                1
            );
            response_error(
                plan.extract_response(&built[0], &ResponseMeta { body_bytes: 0, ..m }, "", &ctx),
                INVALID_JSON,
            );
        }
    }
}

const AMBIGUOUS: &str = "repeated structural property with noResultsMessage";
/// `type: html` alternatives are rejected earlier by the pinned schema enumeration
/// (json/xml); RequestPlan tests cover them below that layer.
const RESPONSES: [&str; 5] = [
    "{type: json, noResultsMessage: \"\"}",
    "{type: json}",
    "{type: json, noResultsMessage: NoTorrents}",
    "{type: xml}",
    "{type: xml, noResultsMessage: \"\"}",
];

#[test]
fn repeated_path_response_blocks_reject_in_either_order() {
    let policy = RESPONSES[0];
    for other in RESPONSES {
        for (first, second) in [(policy, other), (other, policy)] {
            let source = definition(&format!(
                "    - path: api\n      response: {first}\n      response: {second}\n"
            ));
            let e = compile_parsed(&source).err().unwrap();
            assert_eq!(e.path, "$.search", "{first} / {second}");
            assert_eq!(e.message, format!("$.search.paths[0]: {AMBIGUOUS}"));
        }
        let source = definition(&format!(
            "    - path: api\n      response: {policy}\n      response: {{type: html}}\n"
        ));
        assert!(compile_parsed(&source).is_err());
    }
    // Other repeated path properties next to a policy are ambiguous too.
    let source = definition(&format!(
        "{PLAIN}    - path: api\n      path: other\n      response: {policy}\n"
    ));
    let e = compile_parsed(&source).err().unwrap();
    assert_eq!(e.message, format!("$.search.paths[1]: {AMBIGUOUS}"));
}

#[test]
fn repeated_path_lists_and_search_blocks_reject_in_either_order() {
    let with_policy = definition(POLICY);
    let ordinary = definition(PLAIN);
    let lists = |source: &str| {
        let start = source.find("  paths:\n").unwrap();
        let end = source.find("  rows:").unwrap();
        source[start..end].to_owned()
    };
    let search = |source: &str| {
        let start = source.find("search:\n").unwrap();
        source[start..].to_owned()
    };
    for (first, second) in [(&with_policy, &ordinary), (&ordinary, &with_policy)] {
        // Plan::compile's structural allowlist rejects repeated search keys before requests.
        let source =
            with_policy.replacen(&lists(&with_policy), &(lists(first) + &lists(second)), 1);
        let e = compile_parsed(&source).err().unwrap();
        assert_eq!(
            (e.path.as_str(), e.message.as_str()),
            ("$.search.paths", "Ambiguous repeated structural property")
        );
        let source = format!("{first}{}", search(second));
        let e = compile_parsed(&source).err().unwrap();
        assert_eq!(
            (e.path.as_str(), e.message.as_str()),
            ("$.search", "Ambiguous repeated structural property")
        );
    }
}

#[test]
fn policy_plans_keep_repeated_inputs_and_ordinary_duplicates_are_unchanged() {
    let source = definition(
        "    - path: api\n      inputs: {b: 3, b: 4}\n      response: {type: json, noResultsMessage: \"\"}\n",
    )
    .replace("search:\n", "search:\n  inputs: {a: 1, a: 2}\n");
    let plan = compile_parsed(&source).unwrap();
    let (built, _) = requests(&plan);
    assert!(built[0].empty_no_results());
    assert!(
        built[0].url.ends_with("/api?a=1&a=2&b=3&b=4"),
        "{}",
        built[0].url
    );
    // Without any policy, a repeated response block keeps the existing first-match result.
    let source =
        definition("    - path: api\n      response: {type: json}\n      response: {type: xml}\n");
    let plan = compile_parsed(&source).unwrap();
    let (built, _) = requests(&plan);
    assert_eq!(built[0].response_type, "json");
    assert!(!built[0].empty_no_results());
}
