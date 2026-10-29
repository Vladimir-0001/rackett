//! Configured row selector values. Source: Jackett fe8876d2 `CardigannIndexer.cs:438-478`
//! (flat single-line if/else resolved first) and `:517-530` (raw simple-variable
//! substitution); AngleSharp 9253950e `CssSelectorConstructor.cs:958-998`
//! (`ContainsFunctionState` keeps the last ident or string token, so raw multi-token values
//! change meaning upstream and are rejected here). The pinned 1337x definition is unchanged
//! and stays download-blocked; fixtures are synthetic. No live responses or credentials.
use axum::{routing::get, Router};
use chrono::{TimeZone, Utc};
use rackett::{
    builtin::{self, Runtime},
    builtin_runtime,
    cardigann::{self, parse, Limits, Node},
    cardigann_exec::{ExecutionError, Plan},
    models::{Indexer, Release, SearchQuery},
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Instant,
};

const SECRET: &str = "Hidden Uploader Secret";
const FIXED_VALUE: &str =
    "row selector configuration value must be exactly one CSS identifier or quoted string";
const FIXED_TYPE: &str = "row selector configuration value must be text";
const FIXED_POSITION: &str =
    "row selector configuration value must be the whole argument of :contains(...)";

fn source(name: &str) -> String {
    std::fs::read_to_string(format!("vendor/jackett/definitions/{name}.yml")).unwrap()
}
fn compile(source: &str) -> Result<Plan, ExecutionError> {
    Plan::compile(&parse(source, Limits::default()).unwrap())
}
fn pinned(name: &str) -> Plan {
    compile(&source(name)).unwrap()
}
fn settings(values: &[(&str, Value)]) -> BTreeMap<String, Value> {
    values
        .iter()
        .map(|(k, v)| ((*k).into(), v.clone()))
        .collect()
}
fn uploader(value: impl Into<Value>) -> BTreeMap<String, Value> {
    settings(&[("uploader", value.into())])
}
fn context(
    plan: &Plan,
    settings: &BTreeMap<String, Value>,
) -> rackett::cardigann_exec::template::Context {
    plan.context(
        settings,
        &SearchQuery::default(),
        Utc.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap(),
    )
}
fn run(
    plan: &Plan,
    settings: &BTreeMap<String, Value>,
    body: &str,
) -> Result<Vec<Release>, ExecutionError> {
    let ctx = context(plan, settings);
    let request = plan.build_requests(&ctx)?.remove(0);
    plan.extract(&request, body, &ctx)
}
fn titles(releases: &[Release]) -> Vec<&str> {
    releases.iter().map(|r| r.title.as_str()).collect()
}
fn row(title: &str, user: &str) -> String {
    format!(
        "<tr><td class=\"coll-1 name\"><a href=\"/sub/42/0/\" class=\"icon\">c</a>\
         <a href=\"/torrent/1/{title}/\">{title}</a></td><td class=\"coll-2 seeds\">5</td>\
         <td class=\"coll-3 leeches\">1</td><td class=\"coll-date\">Apr. 18th '11</td>\
         <td class=\"coll-4 size\">1.5 GB</td><td class=\"coll-5 user\"><a href=\"/user/x/\">{user}</a></td></tr>"
    )
}
/// Synthetic 1337x-shaped listing; header rows lack torrent links.
fn listing() -> String {
    format!(
        "<!DOCTYPE html><html><body><table><thead><tr><th>name</th></tr></thead><tbody>{}{}{}{}{}</tbody></table></body></html>",
        row("Alpha", "Alice"),
        row("Beta", "Bob Smith"),
        row("Gamma", "alice"),
        row("Delta", "Zoë"),
        row("Epsilon", "ok)")
    )
}
fn assert_hidden(text: &str) {
    for needle in [SECRET, "Hidden", "Secret"] {
        assert!(!text.contains(needle), "{text}");
    }
}

#[test]
fn unchanged_1337x_compiles_with_only_the_admitted_download_blocker() {
    let plan = pinned("1337x");
    let blockers: Vec<_> = plan
        .blockers()
        .iter()
        .map(|b| (b.path.as_str(), b.feature.as_str()))
        .collect();
    assert_eq!(blockers, [("$.download", "download-resolution")]);
    let entry = builtin::catalog()
        .iter()
        .find(|e| e["id"] == "1337x")
        .unwrap();
    assert_eq!(entry["ready"], true);
    assert!(entry["blocked_reason"].is_null());
    // Same context, same upstream semantics.
    let clone = pinned("torrentgalaxyclone");
    assert_eq!(clone.blockers().len(), 1);
    assert_eq!(clone.blockers()[0].path, "$.download");
    // The Pirate Bay substitutes into a JSON path, outside this HTML-only subset.
    let e = compile(&source("thepiratebay")).err().unwrap();
    assert_eq!(e.path, "$.search.rows.selector");
    assert_eq!(
        e.message,
        "Row selector values are only supported for HTML responses"
    );
}

#[test]
fn absent_empty_and_blank_uploaders_select_the_else_branch() {
    let plan = pinned("1337x");
    let all = ["Alpha", "Beta", "Gamma", "Delta", "Epsilon"];
    for config in [
        BTreeMap::new(),
        uploader(""),
        uploader("   "),
        uploader("\u{a0}\u{3000}"),
        uploader(Value::Null),
    ] {
        let rows = run(&plan, &config, &listing()).unwrap();
        assert_eq!(titles(&rows), all, "{config:?}");
        assert_eq!(rows[1].attributes["description"], "Uploader: Bob Smith");
        plan.check_settings(&config).unwrap();
    }
}

#[test]
fn uploader_filters_rows_with_case_sensitive_contains() {
    let plan = pinned("1337x");
    for (value, expected) in [
        ("Alice", vec!["Alpha"]),
        ("alice", vec!["Gamma"]),
        ("Ali", vec!["Alpha"]),
        ("lice", vec!["Alpha", "Gamma"]),
        ("Mallory", vec![]),
        ("ALICE", vec![]),
        ("Zoë", vec!["Delta"]),
        ("_x", vec![]),
        ("-x", vec![]),
        // Quoted CSS strings are substituted raw, exactly as upstream.
        ("\"Bob Smith\"", vec!["Beta"]),
        ("'Bob Smith'", vec!["Beta"]),
        ("\"ok)\"", vec!["Epsilon"]),
        ("\"Bob, Smith\"", vec![]),
        ("'it\"s'", vec![]),
        // An empty CSS string is one token; its empty needle matches every row upstream.
        ("\"\"", vec!["Alpha", "Beta", "Gamma", "Delta", "Epsilon"]),
    ] {
        plan.check_settings(&uploader(value)).unwrap();
        let rows = run(&plan, &uploader(value), &listing()).unwrap();
        assert_eq!(titles(&rows), expected, "{value}");
    }
}

#[test]
fn invalid_values_fail_before_requests_with_fixed_errors() {
    let plan = pinned("1337x");
    for value in [
        SECRET,
        "Two Words",
        " Alice",
        "Alice ",
        "123",
        "1abc",
        "-1",
        "--x",
        "-",
        "a)b",
        "x):not(*",
        "x),tr",
        "x,tr",
        "a.b",
        "#id",
        "*",
        "url(x)",
        "f(x)",
        "a/**/",
        "/**/a",
        "\"unterminated",
        "'mixed\"",
        "\"a\"b\"",
        "\"a\" \"b\"",
        "\"a\"b",
        "a\\62",
        "a\\ b",
        "\"a\\\"b\"",
        "a\nb",
        "a\tb",
        "a\rb",
        "\"a\nb\"",
        "a\u{2028}b",
        "a\u{85}b",
        "a\u{0}b",
        "\"a\u{FFFF}\"",
        "a\u{FFFF}",
        "{{ .Config.sort }}",
    ] {
        for (label, err) in [
            ("build", {
                let ctx = context(&plan, &uploader(value));
                plan.build_requests(&ctx).err()
            }),
            ("settings", plan.check_settings(&uploader(value)).err()),
        ] {
            let err = err.unwrap_or_else(|| panic!("{label} accepted {value:?}"));
            assert_eq!(err.path, "$.search.rows.selector");
            assert_eq!(err.message, FIXED_VALUE, "{value:?}");
            assert_hidden(&err.to_string());
            assert_hidden(&format!("{err:?}"));
        }
    }
    // Wrong JSON types are never coerced (no number or boolean guessing).
    for value in [
        json!(true),
        json!(false),
        json!(5),
        json!(["Alice"]),
        json!({"a": "b"}),
    ] {
        let err = plan.check_settings(&uploader(value.clone())).unwrap_err();
        assert_eq!(err.message, FIXED_TYPE, "{value}");
        let ctx = context(&plan, &uploader(value.clone()));
        assert_eq!(plan.build_requests(&ctx).unwrap_err().message, FIXED_TYPE);
    }
    // Extraction rechecks a context that never went through request building.
    let good = context(&plan, &uploader("Alice"));
    let request = plan.build_requests(&good).unwrap().remove(0);
    let bad = context(&plan, &uploader(SECRET));
    let err = plan.extract(&request, &listing(), &bad).unwrap_err();
    assert_eq!(err.message, FIXED_VALUE);
    assert_hidden(&format!("{err} {err:?}"));
    // Rendered values cannot expand the selector budget.
    let long = "a".repeat(4096);
    let err = plan.check_settings(&uploader(long.as_str())).unwrap_err();
    assert_eq!(err.message, "CSS selector length limit exceeded");
}

#[test]
fn checkbox_true_and_other_settings_keep_existing_coercion() {
    // Only the substituted key bypasses the checkbox mapping; 1337x's own checkbox
    // still renders through its request paths.
    let plan = pinned("1337x");
    let config = settings(&[("disablesort", json!(true)), ("uploader", json!("Alice"))]);
    let ctx = context(&plan, &config);
    assert_eq!(ctx.values[".Config.disablesort"], json!("True"));
    assert_eq!(ctx.values[".Config.uploader"], json!("Alice"));
    assert_eq!(
        context(&plan, &uploader(true)).values[".Config.uploader"],
        json!(true)
    );
    assert_eq!(titles(&run(&plan, &config, &listing()).unwrap()), ["Alpha"]);
}

// ---- static validation of the bounded template subset ----

fn definition(selector: &str, settings: &str, response: &str) -> String {
    format!(
        r#"id: fixture
name: Fixture
description: Synthetic
language: en-US
type: public
encoding: UTF-8
links: [https://example.test/]
caps:
  categories: {{Other: Other}}
  modes: {{search: [q]}}
settings:
{settings}
search:
  paths: [{{path: search{response}}}]
  rows:
    selector: {selector}
  fields:
    category: {{text: Other}}
    size: {{text: 0}}
    seeders: {{text: 0}}
    leechers: {{text: 0}}
    date: {{text: 2024-02-29}}
    download: {{text: /download}}
    title: {{selector: a}}
"#,
        selector = serde_json::to_string(selector).unwrap()
    )
}
const TEXT_U: &str = "  - {name: u, type: text}\n  - {name: v, type: text}\n  - {name: c, type: checkbox, default: true}";
fn synthetic(selector: &str) -> Result<Plan, ExecutionError> {
    compile(&definition(selector, TEXT_U, ""))
}
fn rejected(selector: &str) -> String {
    let e = synthetic(selector)
        .err()
        .unwrap_or_else(|| panic!("accepted {selector}"));
    assert_eq!(e.path, "$.search.rows.selector", "{selector}");
    e.message
}

#[test]
fn values_are_admitted_only_as_the_whole_contains_argument() {
    for selector in [
        "tr{{ if .Config.u }}:contains({{ .Config.u }}){{ else }}{{ end }}",
        "tr{{ if .Config.u }}:CONTAINS({{ .Config.u }}){{ else }}{{ end }}",
        "tr{{ if .Config.u }}:not(:contains({{ .Config.u }})){{ else }}{{ end }}",
        "tr{{ if .Config.u }}:has(td:contains({{ .Config.u }})){{ else }}{{ end }}",
        "tr{{ if .Config.u }}:is(td:contains({{ .Config.u }})){{ else }}{{ end }}",
        "tr{{ if .Config.u }}:where(:contains({{ .Config.u }})){{ else }}{{ end }}",
        "tr{{ if .Config.u }}:contains({{ .Config.u }}), p:contains({{ .Config.u }}){{ else }}{{ end }}",
        "{{ if .Config.u }}tr:contains({{ .Config.u }}){{ else }}tr{{ end }}{{ if .Config.v }}:contains({{ .Config.v }}){{ else }}{{ end }}",
    ] {
        synthetic(selector).unwrap_or_else(|e| panic!("{selector}: {e}"));
    }
    for selector in [
        "a[title=\"{{ .Config.u }}\"]",
        "a[title={{ .Config.u }}]",
        "a[{{ .Config.u }}]",
        "a.{{ .Config.u }}",
        "a#{{ .Config.u }}",
        "a {{ .Config.u }}",
        "{{ .Config.u }}",
        "a:contains(\"{{ .Config.u }}\")",
        "a:contains('{{ .Config.u }}')",
        "a:contains(x{{ .Config.u }})",
        "a:contains({{ .Config.u }}x)",
        "a:contains(-{{ .Config.u }})",
        "a:contains( {{ .Config.u }})",
        "a:contains({{ .Config.u }} )",
        "a:contains(/**/{{ .Config.u }})",
        "a:contains({{ .Config.u }}/**/)",
        "a:contains({{ .Config.u }}{{ .Config.u }})",
        "a/* :contains({{ .Config.u }}) */",
        "a:not({{ .Config.u }})",
        "a:has({{ .Config.u }})",
        "a:nth-child({{ .Config.u }})",
        "a:nth-child(2n of :contains({{ .Config.u }}))",
        "a:lang({{ .Config.u }})",
        "a:foo(:contains({{ .Config.u }}))",
        "a :contains({{ .Config.u }}",
        "a(:contains({{ .Config.u }}))",
        "a contains({{ .Config.u }})",
        "a[x=\":contains({{ .Config.u }})\"]",
    ] {
        let wrapped = format!("tr{{{{ if .Config.u }}}}{selector}{{{{ else }}}}{{{{ end }}}}");
        assert_eq!(rejected(&wrapped), FIXED_POSITION, "{wrapped}");
    }
}

#[test]
fn template_shape_and_branch_rules_are_enforced() {
    let guard = "row selector configuration values must be guarded by their own if block";
    let flat = "row selectors require flat single-line if/else blocks";
    for (selector, message) in [
        ("tr:contains({{ .Config.u }})", guard),
        ("tr{{ if .Config.u }}:contains({{ .Config.v }}){{ else }}{{ end }}", guard),
        ("tr{{ if .Config.u }}{{ else }}:contains({{ .Config.u }}){{ end }}", guard),
        ("tr{{ if .Config.u }}:contains({{ .Config.u }}){{ end }}", flat),
        ("tr{{ if .Config.u }}:contains({{ .Config.u }})\n{{ else }}{{ end }}", flat),
        ("tr{{ if .Config.u }}\r:contains({{ .Config.u }}){{ else }}{{ end }}", flat),
        (
            "tr{{ if .Config.u }}{{ if .Config.v }}:contains({{ .Config.u }}){{ else }}{{ end }}{{ else }}{{ end }}",
            flat,
        ),
        (
            "tr{{ if .Config.u }}:contains({{ re_replace .Config.u \"a\" \"b\" }}){{ else }}{{ end }}",
            flat,
        ),
        (
            "tr{{ if .Config.u }}:contains({{ .Keywords }}){{ else }}{{ end }}",
            guard,
        ),
        (
            "tr{{ if .Config.u }}{{ range .Config.u }}{{ . }}{{ end }}{{ else }}{{ end }}",
            flat,
        ),
        (
            "tr{{ if eq .Config.u \"x\" }}:contains({{ .Config.u }}){{ else }}{{ end }}",
            "row selector conditions must be bare configuration variables",
        ),
        (
            "tr{{ if (.Config.u) }}:contains({{ .Config.u }}){{ else }}{{ end }}",
            "row selector conditions must be bare configuration variables",
        ),
        (
            "tr{{ if .Keywords }}:contains({{ .Keywords }}){{ else }}{{ end }}",
            "row selector conditions must be bare configuration variables",
        ),
        (
            "tr{{ if .Config.c }}:contains({{ .Config.c }}){{ else }}{{ end }}",
            "Row selector values must come from text settings",
        ),
        (
            "tr{{ if .Config.missing }}:contains({{ .Config.missing }}){{ else }}{{ end }}",
            "Row selector values must come from text settings",
        ),
        (
            "tr{{ if .Config.u.x }}:contains({{ .Config.u.x }}){{ else }}{{ end }}",
            "row selector configuration value names must be plain",
        ),
    ] {
        assert_eq!(rejected(selector), message, "{selector}");
    }
    // Both branches are validated, whatever the defaults select.
    let e = rejected("tr{{ if .Config.u }}:contains({{ .Config.u }}){{ else }}:bogus{{ end }}");
    assert!(e.starts_with("unsupported CSS selector"), "{e}");
    let e =
        rejected("tr{{ if .Config.u }}:has(:contains({{ .Config.u }})):bogus{{ else }}{{ end }}");
    assert!(e.starts_with("unsupported CSS selector"), "{e}");
    // Non-text settings types and JSON responses are outside the subset.
    for settings in [
        "  - {name: u, type: password}",
        "  - {name: u, type: select, options: {a: a}}",
        "  - {name: u, type: multi-select, options: {a: a}}",
    ] {
        let e = compile(&definition(
            "tr{{ if .Config.u }}:contains({{ .Config.u }}){{ else }}{{ end }}",
            settings,
            "",
        ))
        .err()
        .unwrap();
        assert_eq!(
            e.message,
            "Row selector values must come from text settings"
        );
    }
    let e = compile(&definition(
        "tr{{ if .Config.u }}:contains({{ .Config.u }}){{ else }}{{ end }}",
        TEXT_U,
        ", response: {type: json}",
    ))
    .err()
    .unwrap();
    assert_eq!(
        e.message,
        "Row selector values are only supported for HTML responses"
    );
}

#[test]
fn count_length_and_nesting_bounds_still_apply() {
    let block = "{{ if .Config.u }}:not(:contains({{ .Config.u }})){{ else }}{{ end }}";
    synthetic(&format!("tr{}", block.repeat(6))).unwrap();
    assert_eq!(
        rejected(&format!("tr{}", block.repeat(7))),
        "row selector variant count limit exceeded"
    );
    // Stand-ins count toward the static length bound.
    let pad = "tr".to_owned() + &":not(p)".repeat(579);
    synthetic(&format!("{pad}{block}")).unwrap();
    assert_eq!(
        rejected(&format!("{pad}:not(p){block}")),
        "row selector length limit exceeded"
    );
    let deep = format!(
        "tr{{{{ if .Config.u }}}}{}:contains({{{{ .Config.u }}}}){}{{{{ else }}}}{{{{ end }}}}",
        ":not(".repeat(32),
        ")".repeat(32)
    );
    assert_eq!(rejected(&deep), "CSS selector nesting limit exceeded");
}

#[test]
fn literal_branches_keep_their_behavior() {
    let settings = "  - {name: c, type: checkbox, default: false}\n  - {name: m, type: multi-select, options: {a: a}, defaults: []}";
    let plan = compile(&definition(
        "tr{{ if .Config.c }}.on{{ else }}.off{{ end }}{{ if .Config.m }}[data-m]{{ else }}{{ end }}",
        settings,
        "",
    ))
    .unwrap();
    let body =
        "<table><tr class=on data-m><td><a>On M</a></td></tr><tr class=on><td><a>On</a></td></tr>\
                <tr class=off><td><a>Off</a></td></tr></table>";
    let get = |config: BTreeMap<String, Value>| {
        plan.check_settings(&config).unwrap();
        titles(&run(&plan, &config, body).unwrap())
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(get(BTreeMap::new()), ["Off"]);
    assert_eq!(get(settings_of("c", json!(true))), ["On M", "On"]);
    assert_eq!(get(settings_of("c", json!(false))), ["Off"]);
    let both = BTreeMap::from([("c".into(), json!(true)), ("m".into(), json!(["a"]))]);
    assert_eq!(get(both), ["On M"]);
    // Rejected literal variants keep their earlier diagnostics.
    let e = compile(&definition(
        "tr{{ if .Config.c }}.on{{ end }}",
        settings,
        "",
    ))
    .err()
    .unwrap();
    assert_eq!(
        e.message,
        "row selectors require flat single-line if/else blocks"
    );
}
fn settings_of(key: &str, value: Value) -> BTreeMap<String, Value> {
    BTreeMap::from([(key.into(), value)])
}

// ---- direct runtime: invalid values never reach the network ----

fn loopback_1337x(base: &str) -> Plan {
    let mut root: Node = parse(&source("1337x"), Limits::default()).unwrap();
    if let cardigann::Value::Mapping(pairs) = &mut root.value {
        for (k, v) in pairs {
            if k.text() == Some("links") {
                let mut link = v.items().unwrap()[0].clone();
                if let cardigann::Value::Scalar(s) = &mut link.value {
                    s.text = base.into();
                }
                v.value = cardigann::Value::Sequence(vec![link]);
            }
        }
    }
    Plan::compile(&root).unwrap()
}
async fn serve(body: String) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = hits.clone();
    let app = Router::new().fallback(get(move || {
        seen.fetch_add(1, Ordering::SeqCst);
        let body = body.clone();
        async move { ([("content-type", "text/html; charset=utf-8")], body) }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, hits, server)
}
fn runtime(plan: Plan, value: Value) -> Runtime {
    Runtime {
        plan: Arc::new(plan),
        settings: uploader(value),
        delay_ms: 0,
        modes: BTreeMap::from([("search".into(), vec!["q".into()])]),
        empty_html_container: None,
        download: None,
    }
}
fn indexer(url: &str, settings: Value) -> Indexer {
    Indexer {
        id: "fixture-1337x".into(),
        name: "1337x fixture".into(),
        kind: "cardigann".into(),
        url: url.into(),
        request_delay_ms: 0,
        definition: Some(json!({"builtin_id": "1337x", "settings": settings})),
        ..Default::default()
    }
}

#[tokio::test]
async fn direct_runtime_rejects_values_before_upstream_io() {
    let (url, hits, server) = serve(listing()).await;
    let plan = || loopback_1337x(&url);
    let client = reqwest::Client::new();
    let i = indexer(&url, json!({}));
    for value in [
        json!(SECRET),
        json!("x),tr"),
        json!("\"a\" \"b\""),
        json!(5),
        json!(true),
    ] {
        let err = builtin_runtime::search(
            &runtime(plan(), value.clone()),
            &client,
            &i,
            &SearchQuery::default(),
            &mut Instant::now(),
        )
        .await
        .unwrap_err();
        assert_eq!(hits.load(Ordering::SeqCst), 0, "{value}");
        assert_hidden(&format!("{err} {err:?} {err:#}"));
    }
    // A valid value reaches the loopback upstream for every pinned path and filters rows.
    let rows = builtin_runtime::search(
        &runtime(plan(), json!("Alice")),
        &client,
        &i,
        &SearchQuery::default(),
        &mut Instant::now(),
    )
    .await
    .unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 4);
    assert_eq!(titles(&rows), ["Alpha"; 4]);
    server.abort();
}

#[test]
fn source_compile_accepts_valid_settings_and_never_echoes_invalid_values() {
    assert!(Runtime::compile(&indexer("https://1337x.to/", json!({"uploader":"Alice"}))).is_ok());
    let err = Runtime::compile(&indexer("https://1337x.to/", json!({"uploader":SECRET})))
        .err()
        .unwrap();
    assert_hidden(&format!("{err} {err:?}"));
}
