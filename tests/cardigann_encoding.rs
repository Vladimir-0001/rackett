//! Offline legacy definition encodings (pinned CardigannIndexer Encoding use).
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
fn non_utf8_rejects_urlencode_and_compiles_urldecode_at_every_filter_path() {
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
            for encoding in ["iso-8859-1", "windows-1255", "tis-620"] {
                let compiled = Plan::compile(&definition(encoding, &search, &fields));
                if name == "urldecode" {
                    assert!(compiled.is_ok(), "{encoding} {path}");
                    continue;
                }
                let error = compiled.err().unwrap();
                assert_eq!(error.path, path);
                assert_eq!(
                    error.message,
                    "urlencode with non-UTF-8 definition encoding is unsupported"
                );
            }
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

/// Titles of `<h1>` rows after a `urldecode` field filter, as `WebUtilityHelpers.UrlDecode`.
fn urldecoded_titles(encoding: &str, titles: &[&str]) -> Vec<String> {
    let plan = Plan::compile(&definition(
        encoding,
        "",
        "      filters: [{name: urldecode}]\n",
    ))
    .unwrap();
    let query = SearchQuery {
        params: BTreeMap::from([("t".into(), "search".into())]),
    };
    let ctx = plan.context(&BTreeMap::new(), &query, Utc::now());
    let request = &plan.build_requests(&ctx).unwrap()[0];
    let body: String = titles
        .iter()
        .map(|t| format!("<article><h1>{t}</h1></article>"))
        .collect();
    plan.extract(request, &body, &ctx)
        .unwrap()
        .into_iter()
        .map(|r| r.title)
        .collect()
}

#[test]
fn urldecode_uses_the_definition_encoding_like_web_utility_helpers() {
    // GetBytes, then WebUtility.UrlDecodeToBytes ('+' is a space, '%' needs two hex digits),
    // then GetString with the same encoding.
    assert_eq!(
        urldecoded_titles(
            "iso-8859-1",
            &["Caf%E9+%C3%A9", "100%+%zz%4", "%41%2b%25", "x%"]
        ),
        ["Café Ã©", "100% %zz%4", "A+%", "x%"]
    );
    assert_eq!(
        urldecoded_titles("UTF-8", &["Caf%C3%A9+%E9"]),
        ["Café \u{fffd}"]
    );
    // Latin1Encoding's EncoderLatin1BestFitFallback: U+0100 is 'A', fullwidth '%' is '%',
    // a character without a best fit is '?', and a surrogate pair is "??".
    assert_eq!(
        urldecoded_titles("iso-8859-1", &["\u{100}\u{ff05}41 \u{4e2d} \u{1f600}"]),
        ["AA ? ??"]
    );
    // windows-1255, including the bytes .NET maps outside the Unicode vendor table.
    assert_eq!(
        urldecoded_titles("windows-1255", &["%F9%EC%E5%ED+%CA%D9%FF%A4"]),
        ["\u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{5ba}\u{f88d}\u{f896}\u{20aa}"]
    );
    assert_eq!(
        urldecoded_titles("tis-620", &["%E4%B7%C2+%DB%80"]),
        ["\u{e44}\u{e17}\u{e22} \u{f8c1}\u{20ac}"]
    );
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
fn pinned_legacy_code_page_definitions_compile() {
    let compile = |id: &str| {
        Plan::compile(
            &load(
                format!("vendor/jackett/definitions/{id}.yml").as_ref(),
                Limits::default(),
            )
            .unwrap(),
        )
    };
    for (id, charset) in [
        ("aftershock", Charset::Latin1),
        ("deildu", Charset::Latin1),
        ("karagarga", Charset::Latin1),
        ("newheaven", Charset::Latin1),
        ("thenewheaven", Charset::Latin1),
        ("tribalmixes", Charset::Latin1),
        ("fuzer", Charset::Windows1255),
        ("siambit", Charset::Windows874),
        ("unlimitz", Charset::Windows874),
    ] {
        match compile(id) {
            Ok(plan) => {
                assert_eq!(plan.charset(), charset, "{id}");
                assert_eq!(plan.blockers()[0].path, "$.login", "{id}");
            }
            Err(e) => panic!("{id}: {e}"),
        }
    }
}

#[test]
fn legacy_code_page_queries_use_dotnet_bytes_and_reject_unrepresentable_text() {
    let query = |q: &str| SearchQuery {
        params: BTreeMap::from([("q".into(), q.into()), ("t".into(), "search".into())]),
    };
    for (encoding, keywords, url) in [
        (
            "windows-1255",
            "שלום 1",
            "https://example.org/search?q=%F9%EC%E5%ED+1",
        ),
        (
            "windows-874",
            "ไทย",
            "https://example.org/search?q=%E4%B7%C2",
        ),
        ("TIS-620", "ไทย", "https://example.org/search?q=%E4%B7%C2"),
    ] {
        let plan = Plan::compile(&definition(encoding, "", "")).unwrap();
        let ctx = plan.context(&BTreeMap::new(), &query(keywords), Utc::now());
        assert_eq!(plan.build_requests(&ctx).unwrap()[0].url, url, "{encoding}");
        // Text outside the page is an error, never a best-fit or '?' byte.
        let ctx = plan.context(&BTreeMap::new(), &query("\u{100}"), Utc::now());
        assert!(plan.build_requests(&ctx).is_err(), "{encoding}");
    }
}
