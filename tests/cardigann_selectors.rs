//! Synthetic fixtures derived from unchanged pinned definitions and
//! CardigannIndexer.cs:326,1682,1832,2491–2568. No live responses or credentials.
use chrono::{TimeZone, Utc};
use rackett::{
    cardigann::{parse, Limits},
    cardigann_exec::{selector, Plan},
    models::{Release, SearchQuery},
};
use std::{collections::BTreeMap, sync::Arc};

fn pinned(name: &str) -> Plan {
    let source = std::fs::read_to_string(format!("vendor/jackett/definitions/{name}.yml")).unwrap();
    Plan::compile(&parse(&source, Limits::default()).unwrap()).unwrap()
}
fn extract(plan: &Plan, body: &str, freeleech: bool) -> Vec<Release> {
    let context = plan.context(
        &BTreeMap::from([("freeleech".into(), serde_json::json!(freeleech))]),
        &SearchQuery::default(),
        Utc.with_ymd_and_hms(2026, 10, 27, 12, 0, 0).unwrap(),
    );
    let requests = plan.build_requests(&context).unwrap();
    plan.extract(&requests[0], body, &context).unwrap()
}
fn nexus_fixture() -> String {
    let rows = [0, 1, 2]
        .into_iter()
        .map(|i| {
            serde_json::json!({"category":401,"id":i+1,"name":format!("Release {i}"),
            "hash":"0123456789abcdef0123456789abcdef01234567", "cover":"/cover.jpg",
            "small_descr":"Synthetic fixture", "size":1024,"numfiles":1,"seeders":4,
            "leechers":2,"times_completed":9,"added":"2026-09-19 16:50",
            "promotion_info":{"down_multiplier":(match i { 0 => serde_json::json!(0), 1 => serde_json::json!(0.5), _ => serde_json::json!(1) }),"up_multiplier":1}})
        })
        .collect::<Vec<_>>();
    serde_json::json!({"data":{"data": rows}}).to_string()
}

#[test]
fn unchanged_nexus_conditional_json_rows_and_concurrent_contexts() {
    let plan = Arc::new(pinned("13city-api"));
    assert!(!plan.blockers().is_empty()); // Login remains an integration blocker.
    let fixture = nexus_fixture();
    assert_eq!(extract(&plan, &fixture, false).len(), 3);
    let free = extract(&plan, &fixture, true);
    assert_eq!(free.len(), 1);
    assert_eq!(free[0].title, "Release 0");
    assert_eq!(free[0].size, Some(1024));
    assert_eq!(free[0].seeders, Some(4));
    assert!(free[0].categories.contains(&2000));
    assert_eq!(free[0].published.as_deref(), Some("2026-09-19T08:50:00Z"));
    let decimals = fixture.replace("\"down_multiplier\":0,", "\"down_multiplier\":0.0,");
    assert_eq!(extract(&plan, &decimals, true).len(), 1);
    assert_eq!(extract(&plan, &decimals, false).len(), 3);
    let jobs = (0..8)
        .map(|i| {
            let plan = Arc::clone(&plan);
            let fixture = fixture.clone();
            std::thread::spawn(move || {
                assert_eq!(
                    extract(&plan, &fixture, i % 2 == 0).len(),
                    if i % 2 == 0 { 1 } else { 3 }
                )
            })
        })
        .collect::<Vec<_>>();
    for job in jobs {
        job.join().unwrap();
    }
}

#[test]
fn unchanged_root_array_definition() {
    let plan = pinned("zamundalife");
    let body = r#"[{"category":"Филми","title":"Synthetic Release","link":"magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567","size":4096,"description":"Fixture"}]"#;
    let rows = extract(&plan, body, false);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "Synthetic Release");
    assert_eq!(rows[0].size, Some(4096));
    assert!(rows[0].categories.contains(&2000));
    assert!(extract(&plan, "[]", false).is_empty());
}

#[test]
fn unchanged_order66_html_filters_freeleech_rows() {
    let plan = pinned("order66");
    assert!(!plan.blockers().is_empty());
    // Match the exact selectors and dd-MM-yyHH:mm layout documented in the
    // pinned definition: TextContent joins the date around the br without space.
    let row = |id, badge| {
        format!("<tr><td><a href='torrents.php?cat=56'>Movies</a></td><td><a href='torrents-details.php?id={id}'>Release {id}</a><a href='download.php?id={id}'>Download</a>{badge}<div class=cov-tip><img src='/cover.jpg'></div></td><td><span class=num-s>4</span></td><td><span class=num-l>2</span></td><td>unused</td><td>1 GB</td><td>9</td><td>16-06-26<br>09:08</td></tr>")
    };
    let body = format!(
        "<table class=ttbl><tbody>{}{}</tbody></table>",
        row(1, "<span class=badge-free>Free</span>"),
        row(2, "")
    );
    assert_eq!(extract(&plan, &body, false).len(), 2);
    let rows = extract(&plan, &body, true);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "Release 1");
    assert_eq!(
        rows[0].link,
        "https://executeorder66.info/download.php?id=1"
    );
    assert_eq!(rows[0].size, Some(1_073_741_824));
    assert_eq!(rows[0].seeders, Some(4));
    assert_eq!(rows[0].peers, Some(6));
    assert_eq!(rows[0].published.as_deref(), Some("2026-06-16T09:08:00Z"));
    assert!(rows[0].categories.contains(&2000));
    assert_eq!(rows[0].attributes["downloadvolumefactor"], "0");
}

fn synthetic(selector: &str, json: bool) -> Result<Plan, rackett::cardigann_exec::ExecutionError> {
    synthetic_with_count(selector, json, false)
}
fn synthetic_with_count(
    selector: &str,
    json: bool,
    count: bool,
) -> Result<Plan, rackett::cardigann_exec::ExecutionError> {
    let source = format!("id: selector\nname: Selector\ndescription: Synthetic\nlanguage: en-US\ntype: public\nencoding: UTF-8\nlinks: [https://example.org/]\ncaps:\n  categories: {{Other: Other}}\n  modes: {{search: [q]}}\nsettings:\n  - {{name: freeleech, type: checkbox, label: Freeleech, default: false}}\nsearch:\n  paths: [{{path: search, {}}}]\n  rows: {{selector: {}, {}}}\n  fields:\n    category: {{text: Other}}\n    title: {{text: Fixture}}\n    size: {{text: 0}}\n    seeders: {{text: 0}}\n    date: {{text: now}}\n    download: {{text: /download}}\n", if json { "response: {type: json}" } else { "" }, serde_json::to_string(selector).unwrap(), if count { "count: {selector: count}" } else { "" });
    Plan::compile(&parse(&source, Limits::default()).unwrap())
}

#[test]
fn html_configuration_branch_changes_real_row_membership() {
    let plan = synthetic(
        "article{{ if .Config.freeleech }}:has(b.free){{ else }}{{ end }}",
        false,
    )
    .unwrap();
    let body = "<article><b class=free>free</b></article><article>ordinary</article>";
    assert_eq!(extract(&plan, body, false).len(), 2);
    assert_eq!(extract(&plan, body, true).len(), 1);
}

#[test]
fn every_branch_validated_and_dynamic_or_excessive_selectors_rejected() {
    for (source, json) in [
        (
            "article{{ if .Config.freeleech }}:contains(a b){{ else }}{{ end }}",
            false,
        ),
        ("data{{ if .Config.freeleech }}[*]{{ else }}{{ end }}", true),
        ("{{ .Config.selector }}", false),
        ("{{ if (.Config.freeleech) }}article{{ else }}div{{ end }}", false),
        ("{{ if .Config.freeleech }}data{{ else }}{{ end }}", true),
        ("{{ range .Categories }}article{{end}}", false),
        ("{{ if .Query.Q }}article{{ else }}div{{ end }}", false),
        ("article{{ if .Config.freeleech }}", false),
        ("article{{ if .Config.freeleech }}.free{{ end }}", false),
        ("article{{ if .Config.freeleech }}.free\n{{ else }}{{ end }}", false),
        ("article{{ if .Config.freeleech }}{{ if .Config.freeleech }}.free{{ else }}{{ end }}{{ else }}{{ end }}", false),
    ] {
        let err = synthetic(source, json).err().unwrap();
        assert_eq!(err.path, "$.search.rows.selector", "{source}");
    }
    let variants = format!(
        "article{}",
        "{{ if .Config.freeleech }}.a{{ else }}.b{{ end }}".repeat(7)
    );
    assert!(synthetic(&variants, false)
        .err()
        .unwrap()
        .message
        .contains("variant count"));
    assert!(synthetic(&"a".repeat(4097), false)
        .err()
        .unwrap()
        .message
        .contains("length"));
}

#[test]
fn root_token_remains_single_array_and_unsupported_paths_are_explicit() {
    let value = serde_json::json!([{"name":"row"}]);
    assert_eq!(
        selector::json_select(&value, "$ ").unwrap_err(),
        "unsupported JSONPath character $"
    );
    assert_eq!(selector::json_select(&value, "$").unwrap(), Some(&value));
    assert_eq!(selector::json_rows(&value, "$", None, 1).unwrap().len(), 1);
    assert!(selector::json_rows(&serde_json::json!({"data":[]}), "$", None, 1).is_err());
    assert!(selector::json_rows(&value, "$", None, 0).is_err());
    for path in ["$..name", "$.*", "$[?(@.name)]", "$.name", "$name"] {
        assert!(selector::json_select(&value, path).is_err(), "{path}");
    }
    let too_many = serde_json::json!(vec![serde_json::json!({}); 100_001]);
    assert!(selector::json_rows(&too_many, "$", None, 200_000)
        .unwrap_err()
        .contains("visit limit"));
}

#[test]
fn json_count_short_circuits_before_row_template_and_invalid_json_precedes_it() {
    let plan = synthetic_with_count(
        "data{{ if .Config.freeleech }}:has(name){{ else }}{{ end }}",
        true,
        true,
    )
    .unwrap();
    let mut context = plan.context(
        &BTreeMap::new(),
        &SearchQuery::default(),
        Utc.with_ymd_and_hms(2026, 10, 27, 12, 0, 0).unwrap(),
    );
    let request = plan.build_requests(&context).unwrap().remove(0);
    context.values.remove(".Config.freeleech");
    assert!(plan
        .extract(&request, r#"{"count":0}"#, &context)
        .unwrap()
        .is_empty());
    assert_eq!(
        plan.extract(&request, r#"{"count":1,"data":[]}"#, &context)
            .unwrap_err()
            .path,
        "$.search.rows.selector"
    );
    assert_eq!(
        plan.extract(&request, "not JSON", &context)
            .unwrap_err()
            .path,
        "$.response"
    );
}

#[test]
fn json_double_scalar_formatting_and_explicit_unsupported_filters() {
    // Raw input must be correctly rounded before applying the precision gate;
    // serde_json's fast parser can otherwise turn this into a 15-digit value.
    let raw = r#"{"data":[{"value":758875707635179193e-4}]}"#;
    let parsed: serde_json::Value = serde_json::from_str(raw).unwrap();
    assert_eq!(
        parsed["data"][0]["value"].as_f64(),
        Some(75_887_570_763_517.92)
    );
    let plan = synthetic("data:has(value:contains(.92))", true).unwrap();
    let context = plan.context(
        &BTreeMap::new(),
        &SearchQuery::default(),
        Utc.with_ymd_and_hms(2026, 10, 27, 12, 0, 0).unwrap(),
    );
    let request = plan.build_requests(&context).unwrap().remove(0);
    let error = plan.extract(&request, raw, &context).unwrap_err();
    assert_eq!(error.path, "$.search.rows");
    assert!(error
        .message
        .contains("unsupported .NET JSON floating-point formatting"));
    let node = parse("selector: value", Limits::default()).unwrap();
    let field = selector::Selector::compile(&node).unwrap();
    for (value, expected) in [
        (0.0, "0"),
        (1.0, "1"),
        (0.5, "0.5"),
        (1.25, "1.25"),
        (0.0001, "0.0001"),
    ] {
        assert_eq!(
            field
                .extract_json(&serde_json::json!({"value":value}), &|s| Ok(s.into()))
                .unwrap()
                .as_deref(),
            Some(expected)
        );
    }
    for value in [1e20, 1e-20, 1.2345678901234567] {
        let value = serde_json::json!({"data":[{"value":value}]});
        // Neither has/not nor rows may turn a formatting error into no match.
        for filter in ["data:has(value:contains(.))", "data:not(value:contains(.))"] {
            assert!(selector::json_rows(&value, filter, None, 10)
                .unwrap_err()
                .contains("unsupported .NET JSON floating-point formatting"));
        }
    }
}
