//! Synthetic, offline responses exercised against unchanged pinned definitions.
use chrono::{TimeZone, Utc};
use rackett::{
    cardigann::{self, Limits},
    cardigann_exec::{filters::Filter, Plan},
    models::SearchQuery,
};
use serde_json::json;
use std::{collections::BTreeMap, path::Path};
fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 2, 12, 0, 0).unwrap()
}
fn query(q: &str) -> SearchQuery {
    SearchQuery {
        params: BTreeMap::from([("q".into(), q.into())]),
    }
}
fn plan(extra: &str, response: &str) -> Plan {
    let source = format!(
        r#"id: semantics
name: Semantics
description: Synthetic
language: en-US
type: public
encoding: UTF-8
links: [https://example.org/]
caps:
  categories: {{Other: Other}}
  modes: {{search: [q]}}
search:
  paths: [{{path: search{response_block}}}]
  inputs: {{q: '{{{{ .Keywords }}}}', original: '{{{{ .Query.Keywords }}}}'}}
{extra}
  rows: {{selector: {selector}}}
  fields:
    category: {{text: Other}}
    title: {{text: '{{{{ .Keywords }}}}|{{{{ .Query.Keywords }}}}'}}
    download: {{text: /download}}
    size: {{text: 0}}
    seeders: {{text: 0}}
    leechers: {{text: 0}}
    date: {{text: 2024-02-29}}
"#,
        response_block = if response == "json" {
            ", response: {type: json}"
        } else {
            ""
        },
        selector = if response == "json" {
            "data"
        } else {
            "article"
        }
    );
    Plan::compile(&cardigann::parse(&source, Limits::default()).unwrap()).unwrap()
}
#[test]
fn keyword_pipeline_order_bindings_and_immutable_context() {
    let p = plan(
        r#"  keywordsfilters:
    - {name: diacritics, args: replace}
    - {name: re_replace, args: ['(?i)s([0-9]+)', '$1']}
    - {name: append, args: '{{ .Config.suffix }}:{{ .Query.Q }}'}
"#,
        "html",
    );
    let ctx = p.context(
        &BTreeMap::from([("suffix".into(), json!("!"))]),
        &query("É S2024"),
        now(),
    );
    let prepared = p.prepare_context(&ctx).unwrap();
    assert_eq!(ctx.values[".Keywords"], "É S2024");
    assert_eq!(prepared.values[".Keywords"], "E 2024!:É S2024");
    assert_eq!(prepared.values[".Query.Keywords"], "É S2024");
    assert_eq!(
        p.prepare_context(&prepared).unwrap().values,
        prepared.values
    );
    let requests = p.build_requests(&ctx).unwrap();
    assert_eq!(
        requests[0].url,
        "https://example.org/search?q=E+2024!%3A%C3%89+S2024&original=%C3%89+S2024"
    );
    let releases = p.extract(&requests[0], "<article/>", &ctx).unwrap();
    assert_eq!(releases[0].title, "E 2024!:É S2024|É S2024");
    assert_eq!(ctx.values[".Keywords"], "É S2024");
}
#[test]
fn keyword_filters_cannot_observe_partially_assigned_keywords() {
    let p = plan(
        "  keywordsfilters: [{name: append, args: '{{ .Keywords }}'}]",
        "html",
    );
    let ctx = p.context(&BTreeMap::new(), &query("original"), now());
    assert!(p
        .build_requests(&ctx)
        .unwrap_err()
        .message
        .contains("variable"));
    assert_eq!(ctx.values[".Keywords"], "original");
}
#[test]
fn preprocessing_is_ordered_html_only_and_bounded() {
    let p = plan(
        r#"  preprocessingfilters:
    - {name: replace, args: [item, article]}
    - {name: re_replace, args: ['TOKEN', '{{ .Keywords }}']}
"#,
        "html",
    );
    let ctx = p.context(&BTreeMap::new(), &query("value"), now());
    let req = p.build_requests(&ctx).unwrap().remove(0);
    assert_eq!(
        p.extract(&req, "<item>TOKEN</item>", &ctx).unwrap().len(),
        1
    );
    let error = p
        .extract(&req, &"x".repeat(1024 * 1024 + 1), &ctx)
        .unwrap_err();
    assert_eq!(error.path, "$.search.preprocessingfilters[0]");
    assert!(error.message.contains("limit"));
    let p = plan(
        "  preprocessingfilters: [{name: replace, args: [data, removed]}]",
        "json",
    );
    let ctx = p.context(&BTreeMap::new(), &query("value"), now());
    let req = p.build_requests(&ctx).unwrap().remove(0);
    assert_eq!(p.extract(&req, r#"{"data":[{}]}"#, &ctx).unwrap().len(), 1);
}
#[test]
fn pipeline_expansion_and_regex_compatibility_fail_explicitly() {
    let p = plan(
        "  keywordsfilters: [{name: replace, args: [a, aaaa]}]",
        "html",
    );
    let ctx = p.context(&BTreeMap::new(), &query(&"a".repeat(300000)), now());
    assert!(p
        .build_requests(&ctx)
        .unwrap_err()
        .message
        .contains("output limit"));
    for name in ["regexp", "re_replace"] {
        let args = if name == "regexp" {
            vec!["(?<=secret)value".into()]
        } else {
            vec!["(?<=secret)value".into(), "x".into()]
        };
        assert!(Filter::compile(name, &args).is_err());
    }
    let p = plan("  keywordsfilters: [{name: base64decode}]", "html");
    let ctx = p.context(&BTreeMap::new(), &query("_w"), now());
    assert!(p
        .build_requests(&ctx)
        .unwrap_err()
        .message
        .contains("utf-8"));
}
#[test]
fn dateparse_preserves_original_invalid_input_and_ordinal_pipeline() {
    let p = plan("", "html");
    let ctx = p.context(&BTreeMap::new(), &query(""), now());
    let date = Filter::compile("dateparse", &["yyyy-MM-dd HH:mm".into()]).unwrap();
    for input in ["2023-02-29 12:00", "2024-02-29  12:00", "  malformed  "] {
        assert_eq!(date.apply(input, &ctx).unwrap(), input);
    }
    let ordinal = Filter::compile("re_replace", &["st|nd|rd|th".into(), "".into()]).unwrap();
    let date = Filter::compile("dateparse", &["htt MMM. d".into()]).unwrap();
    let input = ordinal.apply("7am Sep. 14th", &ctx).unwrap();
    assert_eq!(
        date.apply(&input, &ctx).unwrap(),
        "Mon, 14 Sep 2026 07:00:00 +0"
    );
}
#[test]
fn pinned_mvgroup_definition_request_and_response() {
    let n = cardigann::load(
        Path::new("vendor/jackett/definitions/mvgroupmain.yml"),
        Limits::default(),
    )
    .unwrap();
    let p = Plan::compile(&n).unwrap();
    assert!(!p.blockers().is_empty());
    for stripped in [false, true] {
        let ctx = p.context(
            &BTreeMap::from([
                ("stripS01E01".into(), json!(stripped)),
                ("dropbbc".into(), json!(true)),
            ]),
            &query("Documentary S01E01"),
            now(),
        );
        let req = p.build_requests(&ctx).unwrap().remove(0);
        let filter = url::Url::parse(&req.url)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "filter")
            .unwrap()
            .1
            .into_owned();
        assert_eq!(
            filter,
            if stripped {
                "Documentary "
            } else {
                "Documentary S01E01"
            }
        );
        let releases = p
            .extract(
                &req,
                include_str!("fixtures/cardigann-semantics/mvgroupmain.html"),
                &ctx,
            )
            .unwrap();
        let r = &releases[0];
        assert_eq!(
            r.title,
            if stripped {
                "Synthetic Documentary"
            } else {
                "Synthetic Documentary S01E01"
            }
        );
        assert_eq!(r.published.as_deref(), Some("2024-02-29T00:00:00Z"));
        assert_eq!(r.seeders, Some(7));
        assert_eq!(r.peers, Some(10));
        assert_eq!(r.size, Some(536870912));
    }
}
#[test]
fn pinned_onejav_definition_calendar_and_timezone() {
    let n = cardigann::load(
        Path::new("vendor/jackett/definitions/onejav.yml"),
        Limits::default(),
    )
    .unwrap();
    let p = Plan::compile(&n).unwrap();
    let ctx = p.context(&BTreeMap::new(), &query("sample"), now());
    let reqs = p.build_requests(&ctx).unwrap();
    assert_eq!(reqs[0].url, "https://onejav.com/search/sample");
    assert_eq!(reqs[1].url, "https://onejav.com/search/sample?page=2");
    let releases = p
        .extract(
            &reqs[0],
            include_str!("fixtures/cardigann-semantics/onejav.html"),
            &ctx,
        )
        .unwrap();
    assert_eq!(releases[0].title, "Synthetic Release");
    assert_eq!(
        releases[0].published.as_deref(),
        Some("2024-02-29T07:00:00Z")
    );
    assert_eq!(releases[0].size, Some(1073741824));
}

#[test]
fn preprocessing_aggregate_work_and_filter_count_limits() {
    let filters = "    - {name: trim}\n".repeat(32);
    let p = plan(&format!("  preprocessingfilters:\n{filters}"), "html");
    let ctx = p.context(&BTreeMap::new(), &query(""), now());
    let req = p.build_requests(&ctx).unwrap().remove(0);
    let err = p.extract(&req, &"x".repeat(1024 * 1024), &ctx).unwrap_err();
    assert!(err.message.contains("work byte budget"));
    // The pinned schema accepts sequences; semantic compilation imposes the cap.
    let n = cardigann::load(
        Path::new("vendor/jackett/definitions/mvgroupmain.yml"),
        Limits::default(),
    )
    .unwrap();
    let mut n = n;
    if let cardigann::Value::Mapping(root) = &mut n.value {
        let search = root
            .iter_mut()
            .find(|(k, _)| k.text() == Some("search"))
            .unwrap();
        if let cardigann::Value::Mapping(search) = &mut search.1.value {
            let (_, pipeline) = search
                .iter_mut()
                .find(|(k, _)| k.text() == Some("keywordsfilters"))
                .unwrap();
            if let cardigann::Value::Sequence(items) = &mut pipeline.value {
                items.resize(33, items[0].clone());
            }
        }
    }
    let err = Plan::compile(&n).err().unwrap();
    assert_eq!(err.path, "$.search.keywordsfilters");
    assert!(err.message.contains("count limit"));
}
