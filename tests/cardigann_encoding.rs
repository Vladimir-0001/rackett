//! Offline ISO-8859-1 definition encoding subset (pinned CardigannIndexer Encoding use).
use chrono::Utc;
use rackett::{
    cardigann::{load, parse, Limits},
    cardigann_exec::{request::Charset, Plan},
    models::SearchQuery,
};
use std::collections::BTreeMap;

fn definition(encoding: &str, search: &str, fields: &str) -> rackett::cardigann::Node {
    parse(&format!("id: encoded\nname: Encoded\ndescription: Synthetic\nlanguage: en-US\ntype: public\nencoding: {encoding}\nlinks: [https://example.org/]\ncaps:\n  categories: {{Other: Other}}\n  modes: {{search: [q]}}\nsearch:\n  paths: [{{path: search}}]\n  inputs: {{q: '{{{{ .Keywords }}}}'}}\n{search}  rows: {{selector: article}}\n  fields:\n    category: {{text: Other}}\n    seeders: {{text: 0}}\n    date: {{text: 2026-10-27T12:00:00Z}}\n    size: {{text: 0}}\n    download: {{text: /download}}\n    title:\n      selector: h1\n{fields}"), Limits::default()).unwrap()
}

#[test]
fn latin1_rejects_every_url_codec_filter_path_and_utf8_is_unchanged() {
    for name in ["urlencode", "urldecode"] {
        for (search, fields, path) in [
            (
                format!("  keywordsfilters: [{{name: {name}}}]\n"),
                String::new(),
                "$.search.keywordsfilters[0]",
            ),
            (
                format!("  preprocessingfilters: [{{name: trim}}, {{name: {name}}}]\n"),
                String::new(),
                "$.search.preprocessingfilters[1]",
            ),
            (
                String::new(),
                format!("      filters: [{{name: trim}}, {{name: {name}}}]\n"),
                "$.search.fields.title.filters[1]",
            ),
        ] {
            let error = Plan::compile(&definition("iso-8859-1", &search, &fields))
                .err()
                .unwrap();
            assert_eq!(error.path, path);
            assert_eq!(
                error.message,
                format!("{name} with non-UTF-8 definition encoding is unsupported")
            );
            let plan = Plan::compile(&definition("UTF-8", &search, &fields)).unwrap();
            assert_eq!(plan.charset(), Charset::Utf8);
        }
    }
    // Field-level optional/default/case do not run filters; querystring stays UTF-8 upstream.
    let plan = Plan::compile(&definition(
        "ISO-8859-1",
        "  keywordsfilters: [{name: querystring, args: q}]\n",
        "      filters: [{name: querystring, args: q}]\n      case: {'b': x}\n",
    ))
    .unwrap();
    assert_eq!(plan.charset(), Charset::Latin1);
}

#[test]
fn latin1_plan_builds_definition_encoded_query_offline() {
    let plan = Plan::compile(&definition("iso-8859-1", "", "")).unwrap();
    let query = |q: &str| SearchQuery {
        params: BTreeMap::from([("q".into(), q.into()), ("t".into(), "search".into())]),
    };
    let ctx = plan.context(&BTreeMap::new(), &query("café"), Utc::now());
    assert_eq!(
        plan.build_requests(&ctx).unwrap()[0].url,
        "https://example.org/search?q=caf%E9"
    );
    let ctx = plan.context(&BTreeMap::new(), &query("€"), Utc::now());
    let error = plan.build_requests(&ctx).unwrap_err();
    assert_eq!(error.path, "$.search");
    assert!(error.message.contains("ISO-8859-1"));
}

#[test]
fn pinned_latin1_definitions_stay_login_blocked() {
    let compile = |id: &str| {
        Plan::compile(
            &load(
                format!("vendor/jackett/definitions/{id}.yml").as_ref(),
                Limits::default(),
            )
            .unwrap(),
        )
    };
    let error = compile("aftershock").err().unwrap();
    assert_eq!(error.path, "$.search.fields.title.filters[1]");
    assert!(error.message.starts_with("urldecode with non-UTF-8"));
    for id in [
        "deildu",
        "karagarga",
        "newheaven",
        "thenewheaven",
        "tribalmixes",
    ] {
        match compile(id) {
            Ok(plan) => {
                assert_eq!(plan.charset(), Charset::Latin1, "{id}");
                assert_eq!(plan.blockers()[0].path, "$.login", "{id}");
            }
            Err(e) => panic!("{id}: {e}"),
        }
    }
}
