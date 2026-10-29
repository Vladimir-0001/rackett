//! Synthetic fixtures for HTML `search.error` blocks using pinned definitions.
//! Source: Jackett fe8876d2 `CardigannIndexer.cs:538-561` (ordered blocks, document
//! `QuerySelector`, first match throws) and `:1802-1826` (raw document checked before
//! preprocessing); `IndexerDefinition.cs:104-109,148`. AngleSharp v1.8.6
//! (9253950e551f9ed95294f775675de9aa249894a2) `TextContainsMatcher.cs:28-94`,
//! `HtmlTemplateElement.cs:60-94`. No live responses or credentials.
use chrono::{TimeZone, Utc};
use rackett::{
    cardigann::{parse, Diagnostic, Limits},
    cardigann_exec::{ExecutionError, Plan},
    models::{Release, SearchQuery},
};
use std::collections::BTreeMap;

const SOURCE_ERROR: &str = "Source reported a search error";

fn source(name: &str) -> String {
    std::fs::read_to_string(format!("vendor/jackett/definitions/{name}.yml")).unwrap()
}
fn compile(source: &str) -> Result<Plan, ExecutionError> {
    Plan::compile(&parse(source, Limits::default()).unwrap())
}
/// Pinned definition with its error block sequence replaced.
fn with_errors(name: &str, errors: &str) -> String {
    let source = source(name);
    let start = source.find("  error:\n").unwrap();
    let end = start + source[start..].find("\n\n  rows:").unwrap();
    format!("{}{errors}{}", &source[..start], &source[end..])
}
fn run(plan: &Plan, body: &str) -> Result<Vec<Release>, ExecutionError> {
    let query = SearchQuery {
        params: BTreeMap::from([("q".into(), "ubuntu".into()), ("t".into(), "search".into())]),
    };
    let context = plan.context(
        &BTreeMap::new(),
        &query,
        Utc.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap(),
    );
    let requests = plan.build_requests(&context).unwrap();
    plan.extract(&requests[0], body, &context)
}
fn page(head: &str, body: &str) -> String {
    format!("<!DOCTYPE html><html><head>{head}</head><body>{body}</body></html>")
}
const ROW: &str =
    "<article class='resource-card'><a href='/hash/0123456789abcdef0123456789abcdef01234567'>\
    <span>【Video】</span>Ubuntu 24.04 desktop</a><div class='meta'><span>Added：2024-05-01</span>\
    <span>Size：1.5 GB</span></div></article>";
fn source_error(result: Result<Vec<Release>, ExecutionError>, index: usize) {
    let e = result.unwrap_err();
    assert_eq!(e.path, format!("$.search.error[{index}]"));
    assert_eq!(e.message, SOURCE_ERROR);
}
fn response_failure(result: Result<Vec<Release>, ExecutionError>, needle: &str) -> ExecutionError {
    let e = result.unwrap_err();
    assert_eq!(e.path, "$.response", "{e}");
    assert!(e.message.contains(needle), "{e}");
    e
}

#[test]
fn internal_server_error_page_fails_and_valid_rows_succeed() {
    let plan = compile(&source("52bt")).unwrap();
    assert!(plan.blockers().is_empty());
    let releases = run(&plan, &page("", ROW)).unwrap();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].title, "Ubuntu 24.04 desktop");
    assert_eq!(releases[0].size, Some(1610612736));
    source_error(
        run(
            &plan,
            &page(
                "<title>500 Internal Server Error</title>",
                "<h1>Internal Server Error</h1>",
            ),
        ),
        0,
    );
    // An error block wins over rows that would otherwise extract.
    source_error(
        run(
            &plan,
            &page("", &format!("{ROW}<p>Internal Server Error</p>")),
        ),
        0,
    );
    assert!(run(&plan, &page("", "")).unwrap().is_empty());
}

#[test]
fn bluraytracker_no_results_text_is_not_an_error() {
    let plan = compile(&source("bluraytracker")).unwrap();
    assert_eq!(plan.blockers()[0].path, "$.login");
    let none = "<div class='error'>There are no results found.</div>";
    assert!(run(&plan, &page("", none)).unwrap().is_empty());
    assert!(run(&plan, &page("", "")).unwrap().is_empty());
    source_error(
        run(&plan, &page("", "<div class='error'>Login required</div>")),
        0,
    );
    source_error(
        run(
            &plan,
            &page(
                "",
                &format!("{none}<div class='error'>Database unavailable</div>"),
            ),
        ),
        0,
    );
    // The upstream row limit does not apply: many matches are still an error.
    let many = "<div class='error'>x</div>".repeat(1500);
    source_error(run(&plan, &page("", &many)), 0);
    // Inert template contents are outside upstream's ChildNodes.
    let inert = "<template><div class='error'>Login required</div></template>";
    assert!(run(&plan, &page("", inert)).unwrap().is_empty());
}

#[test]
fn contains_is_case_sensitive_and_root_includes_head() {
    let plan = compile(&source("52bt")).unwrap();
    let lower = page("<title>internal server error</title>", ROW);
    assert_eq!(run(&plan, &lower).unwrap().len(), 1);
    // Only the head title carries the text: `:root` is the html element.
    source_error(
        run(&plan, &page("<title>Internal Server Error</title>", ROW)),
        0,
    );
    // Text split across nodes still matches, as AngleSharp's descendant text does.
    source_error(
        run(&plan, &page("", "<p>Internal <b>Server</b> Error</p>")),
        0,
    );
    // Script and style text belong to TextContent; inert template content does not.
    source_error(
        run(
            &plan,
            &page("<script>var e='Internal Server Error';</script>", ROW),
        ),
        0,
    );
    source_error(
        run(
            &plan,
            &page("<style>/* Internal Server Error */</style>", ROW),
        ),
        0,
    );
    let inert = page(
        "",
        &format!("<template>Internal Server Error</template>{ROW}"),
    );
    assert_eq!(run(&plan, &inert).unwrap().len(), 1);
    // Comments are not text.
    let comment = page("", &format!("<!-- Internal Server Error -->{ROW}"));
    assert_eq!(run(&plan, &comment).unwrap().len(), 1);
}

#[test]
fn raw_document_is_checked_before_preprocessing() {
    let filtered = |from: &str, to: &str| {
        source("52bt").replace(
            "  error:\n",
            &format!("  preprocessingfilters:\n    - name: replace\n      args: [\"{from}\", \"{to}\"]\n  error:\n"),
        )
    };
    let removes = compile(&filtered("Internal Server Error", "")).unwrap();
    source_error(
        run(
            &removes,
            &page("", &format!("{ROW}<p>Internal Server Error</p>")),
        ),
        0,
    );
    assert_eq!(run(&removes, &page("", ROW)).unwrap().len(), 1);
    // Text created only by preprocessing is not an error, and rows use the processed body.
    let creates = compile(&filtered("Ubuntu 24.04", "Internal Server Error")).unwrap();
    let releases = run(&creates, &page("", ROW)).unwrap();
    assert_eq!(releases[0].title, "Internal Server Error desktop");
}

#[test]
fn ordered_blocks_report_first_match() {
    let plan = compile(&with_errors(
        "52bt",
        "  error:\n    - selector: :root:contains(\"Service Unavailable\")\n    \
         - selector: :root:contains(\"Connection failed\")\n      message:\n        \
         text: \"Connection failed: Connection refused\"\n    \
         - selector: :root:contains(\"[Anti-DDoS]\")",
    ))
    .unwrap();
    source_error(run(&plan, &page("", "Connection failed [Anti-DDoS]")), 1);
    source_error(run(&plan, &page("", "[Anti-DDoS] Service Unavailable")), 0);
    source_error(run(&plan, &page("", "[Anti-DDoS]")), 2);
    assert_eq!(run(&plan, &page("", ROW)).unwrap().len(), 1);
    let eight =
        "  error:\n".to_owned() + &"    - selector: div.never\n".repeat(7) + "    - selector: html";
    source_error(
        run(
            &compile(&with_errors("52bt", &eight)).unwrap(),
            &page("", ROW),
        ),
        7,
    );
}

#[test]
fn failures_never_display_page_message_selector_or_query() {
    let secret = "MESSAGE-LITERAL-7f3a";
    let plan = compile(&with_errors(
        "52bt",
        &format!(
            "  error:\n    - selector: img[src=\"../images/bladpobierania.png\"]\n      \
             message:\n        text: \"{secret} you do not have permission\""
        ),
    ))
    .unwrap();
    let body = page(
        "<script>document.cookie='uid=1; pass=COOKIE-SECRET';</script>",
        &format!(
            "<p>passkey=PASSKEY-SECRET https://tracker.invalid/?q=ubuntu</p>{}\
             <img src='../images/bladpobierania.png'>",
            "LONGPAGE ".repeat(50_000)
        ),
    );
    let e = run(&plan, &body).unwrap_err();
    let diagnostic = Diagnostic::from(e.clone());
    for text in [e.to_string(), format!("{e:?}"), format!("{diagnostic:?}")] {
        for leaked in [
            secret,
            "permission",
            "COOKIE-SECRET",
            "PASSKEY-SECRET",
            "LONGPAGE",
            "tracker.invalid",
            "ubuntu",
            "bladpobierania",
            "img[",
        ] {
            assert!(!text.contains(leaked), "{leaked} in {text}");
        }
    }
    assert_eq!(e.to_string(), format!("$.search.error[0]: {SOURCE_ERROR}"));
    assert_eq!(diagnostic.path, "$.search.error[0]");
    assert_eq!(diagnostic.message, SOURCE_ERROR);
    // Response-wide failures are static as well.
    let plan = compile(&source("52bt")).unwrap();
    let marked = page("", "<select><option>PASSKEY-SECRET</option></select>");
    let e = response_failure(run(&plan, &marked), "select");
    assert!(!format!("{e} {e:?}").contains("PASSKEY") && !format!("{e}").contains("Internal"));
    // Compile diagnostics may name an invalid selector, not literal messages.
    let e = compile(&with_errors("52bt", "  error:\n    - selector: \"div[\""))
        .err()
        .unwrap();
    assert_eq!(e.path, "$.search.error[0].selector");
    assert!(e.message.contains("div["));
}

#[test]
fn traversal_failures_are_response_errors_not_empty_results() {
    let plan = compile(&source("52bt")).unwrap();
    response_failure(
        run(&plan, &"x".repeat(8 * 1024 * 1024 + 1)),
        "Response byte budget",
    );
    let deep = page(
        "",
        &format!("{}x{}", "<div>".repeat(140), "</div>".repeat(140)),
    );
    response_failure(run(&plan, &deep), "depth limit");
    let wide = page("", &"<b></b>".repeat(100_001));
    response_failure(run(&plan, &wide), "node limit");
    response_failure(
        run(
            &plan,
            &page(
                "",
                "<div><template shadowrootmode='open'><p>x</p></template></div>",
            ),
        ),
        "shadow root",
    );
    // Each nested error div rescans ~300 KB before finding the no-results text at its end.
    let bluray = compile(&source("bluraytracker")).unwrap();
    let nested = page(
        "",
        &format!(
            "{}{}There are no results found.{}",
            "<div class='error'>".repeat(120),
            "a".repeat(300_000),
            "</div>".repeat(120)
        ),
    );
    response_failure(run(&bluray, &nested), "scan budget");
    // The same text in one error div fits the budget.
    let single = page(
        "",
        &format!(
            "<div class='error'>{}There are no results found.</div>",
            "a".repeat(300_000)
        ),
    );
    assert!(run(&bluray, &single).unwrap().is_empty());
}

#[test]
fn compile_rejects_unsupported_error_blocks() {
    let rejected = |errors: &str, path: &str| {
        let e = compile(&with_errors("52bt", errors))
            .err()
            .unwrap_or_else(|| panic!("{errors}"));
        assert_eq!(e.path, path, "{errors}: {e}");
    };
    let one = "$.search.error[0]";
    rejected(
        "  error:\n    - selector: html\n      path: search",
        &format!("{one}.path"),
    );
    rejected(
        "  error:\n    - selector: html\n      unknown: x",
        &format!("{one}.unknown"),
    );
    rejected(
        "  error:\n    - selector: html\n      message:\n        selector: p",
        &format!("{one}.message.selector"),
    );
    rejected(
        "  error:\n    - selector: html\n      message:\n        text: x\n        filters: []",
        &format!("{one}.message.filters"),
    );
    rejected(
        "  error:\n    - selector: html\n      message: literal",
        &format!("{one}.message"),
    );
    rejected(
        "  error:\n    - selector: html\n      message: {}",
        &format!("{one}.message.text"),
    );
    rejected(
        "  error:\n    - selector: html\n      message:\n        text: [a]",
        &format!("{one}.message.text"),
    );
    rejected(
        "  error:\n    - selector: html\n      message:\n        text: \"{{ .Config.x }}\"",
        &format!("{one}.message.text"),
    );
    rejected(
        &format!(
            "  error:\n    - selector: html\n      message:\n        text: {}",
            "m".repeat(4097)
        ),
        &format!("{one}.message.text"),
    );
    rejected(
        "  error:\n    - selector: \"{{ .Config.x }}\"",
        &format!("{one}.selector"),
    );
    rejected(
        "  error:\n    - selector: [html]",
        &format!("{one}.selector"),
    );
    rejected("  error:\n    - selector: \"\"", &format!("{one}.selector"));
    rejected(
        "  error:\n    - selector: \"  \"",
        &format!("{one}.selector"),
    );
    // The pinned schema reports a missing selector on the block.
    rejected("  error:\n    - message:\n        text: x", one);
    rejected(
        "  error:\n    - selector: \"div:unknown(x)\"",
        &format!("{one}.selector"),
    );
    rejected(
        "  error:\n    - selector: \"div:contains(\"",
        &format!("{one}.selector"),
    );
    rejected(
        &format!("  error:\n    - selector: \"{}\"", "a".repeat(4097)),
        &format!("{one}.selector"),
    );
    rejected(
        &format!(
            "  error:\n    - selector: \"{}a{}\"",
            ":not(".repeat(33),
            ")".repeat(33)
        ),
        &format!("{one}.selector"),
    );
    rejected("  error:\n    - html", one);
    rejected("  error: []", "$.search.error");
    rejected("  error:\n    selector: html", "$.search.error");
    rejected("  error: html", "$.search.error");
    rejected(
        ("  error:\n".to_owned() + &"    - selector: html\n".repeat(9)).trim_end(),
        "$.search.error",
    );
    // Duplicates at every new mapping level are ambiguous (or rejected by the parser).
    for errors in [
        "  error:\n    - selector: html\n      selector: body",
        "  error:\n    - selector: html\n      message:\n        text: a\n      message:\n        text: b",
        "  error:\n    - selector: html\n      message:\n        text: a\n        text: b",
        "  error:\n    - selector: html\n  error:\n    - selector: body",
    ] {
        let source = with_errors("52bt", errors);
        if let Ok(node) = parse(&source, Limits::default()) {
            assert!(Plan::compile(&node).is_err(), "{errors}");
        }
    }
    // Exactly eight blocks are accepted.
    let eight = "  error:\n".to_owned() + &"    - selector: html\n".repeat(8);
    assert!(compile(&with_errors("52bt", eight.trim_end())).is_ok());
}

#[test]
fn compile_rejects_error_blocks_with_non_html_requests() {
    let paths = |second: &str| {
        source("52bt").replacen(
            "  paths:\n",
            &format!("  paths:\n    - path: api\n{second}"),
            1,
        )
    };
    let e = compile(&paths("      response:\n        type: json\n"))
        .err()
        .unwrap();
    assert_eq!(e.path, "$.search.error");
    assert!(compile(&paths("")).is_ok());
    // noResultsMessage remains unsupported with or without error blocks.
    // noResultsMessage remains unsupported (the pinned schema requires a type with it).
    let e = compile(&paths(
        "      response:\n        type: json\n        noResultsMessage: none\n",
    ))
    .err()
    .unwrap();
    assert!(e.message.contains("noResultsMessage"), "{e}");
    // Without error blocks the JSON path reaches the established field validation.
    let e = compile(&with_errors("52bt", "").replacen(
        "  paths:\n",
        "  paths:\n    - path: api\n      response:\n        type: json\n",
        1,
    ))
    .err()
    .unwrap();
    assert_eq!(e.path, "$.search.fields.title");
}

#[test]
fn ordinary_plans_without_error_blocks_are_unchanged() {
    let plan = compile(&with_errors("52bt", "")).unwrap();
    assert_eq!(run(&plan, &page("", ROW)).unwrap().len(), 1);
    // Without error blocks this page is ordinary rows-free success.
    assert!(run(&plan, &page("", "Internal Server Error"))
        .unwrap()
        .is_empty());
    // Ordinary failures keep their established paths.
    let deep = page(
        "",
        &format!("{}x{}", "<div>".repeat(140), "</div>".repeat(140)),
    );
    assert_eq!(run(&plan, &deep).unwrap_err().path, "$.search.rows");
}

/// Error selectors spanning the selector kinds that could otherwise short-circuit:
/// plain, nonempty and empty `:contains`, structural `:has`, and the root itself.
const GUARDED_SELECTORS: [&str; 6] = [
    "div.error",
    "span.error",
    "div.error:not(:has(.ok))",
    ":root:contains(\"Login required\")",
    ":root:contains(\"\")",
    "html",
];
fn guarded(selector: &str) -> Plan {
    compile(&with_errors(
        "52bt",
        &format!("  error:\n    - selector: {selector:?}"),
    ))
    .unwrap()
}
/// Documents whose tree differs from upstream, each hiding an error element upstream sees.
const UNSUPPORTED: [(&str, &str); 7] = [
    (
        "<div class='error'><template shadowrootmode='open'><b class='ok'>x</b></template>Login required</div>",
        "shadow root",
    ),
    (
        "<div class='error'><template shadowrootmode='closed'><b class='ok'>x</b></template>Login required</div>",
        "shadow root",
    ),
    (
        "<div class='box'><template shadowrootmode='OPEN'></template><p class='error'>Login required</p></div>",
        "shadow root",
    ),
    (
        "<template><div><template shadowrootmode='bogus'></template></div></template>",
        "shadow root",
    ),
    (
        "<select><option>x<span class='error'>Login required</span></option></select>",
        "select",
    ),
    (
        "<form><select name='cat'><option value='1'>Movies</option></select></form>",
        "select",
    ),
    ("<template><select></select></template>", "select"),
];

#[test]
fn unsupported_documents_fail_closed_for_every_error_selector() {
    for selector in GUARDED_SELECTORS {
        let plan = guarded(selector);
        for (body, reason) in UNSUPPORTED {
            let e = response_failure(run(&plan, &page("", &format!("{body}{ROW}"))), reason);
            let diagnostic = Diagnostic::from(e.clone());
            for text in [e.to_string(), format!("{e:?}"), format!("{diagnostic:?}")] {
                for leaked in ["Login", "Movies", "pixel", "error'", "ok'", selector] {
                    assert!(!text.contains(leaked), "{leaked} in {text}");
                }
            }
            assert_eq!(diagnostic.path, "$.response");
        }
    }
    // `.box > p.error:first-child` would miss after a retained template that upstream removes.
    let first = guarded(".box > p.error:first-child");
    response_failure(
        run(
            &first,
            &page(
                "",
                "<div class='box'><template shadowrootmode='closed'></template><p class='error'>x</p></div>",
            ),
        ),
        "shadow root",
    );
    source_error(
        run(
            &first,
            &page("", "<div class='box'><p class='error'>x</p></div>"),
        ),
        0,
    );
}

#[test]
fn unsupported_documents_are_checked_before_preprocessing() {
    let plan = compile(&source("52bt").replace(
        "  error:\n    - selector: :root:contains(\"Internal Server Error\")",
        "  preprocessingfilters:\n    - name: re_replace\n      args: [\"<(noscript|select|template)[^>]*>\", \"\"]\n  \
         error:\n    - selector: div.error",
    ))
    .unwrap();
    for (body, reason) in UNSUPPORTED {
        response_failure(run(&plan, &page("", &format!("{body}{ROW}"))), reason);
    }
    assert_eq!(run(&plan, &page("", ROW)).unwrap().len(), 1);
}

#[test]
fn noscript_content_is_parsed_as_markup_like_upstream() {
    // Jackett parses with scripting disabled, so noscript content is markup.
    for selector in GUARDED_SELECTORS {
        let plan = guarded(selector);
        let hidden =
            "<noscript><div class='error'><span class='error'>Login required</span></div></noscript>";
        source_error(run(&plan, &page("", &format!("{hidden}{ROW}"))), 0);
        // Analytics noscript elsewhere neither fails nor stops matching.
        let always = matches!(selector, ":root:contains(\"\")" | "html");
        for (head, body) in [
            (
                "",
                "<noscript><img src='/pixel.gif' alt='Login required'></noscript>",
            ),
            ("<noscript><link rel='x'></noscript>", ""),
            ("<noscript>Login required</noscript>", ""),
        ] {
            let result = run(&plan, &page(head, &format!("{body}{ROW}")));
            if always || (selector.starts_with(":root") && head.contains("Login")) {
                source_error(result, 0);
            } else {
                assert_eq!(result.unwrap().len(), 1, "{selector} {head} {body}");
            }
        }
    }
    // Selectors without :contains see elements inside noscript too.
    let image = compile(&with_errors(
        "52bt",
        "  error:\n    - selector: img[src=\"../images/bladpobierania.png\"]",
    ))
    .unwrap();
    source_error(
        run(
            &image,
            &page(
                "",
                "<noscript><img src='../images/bladpobierania.png'></noscript>",
            ),
        ),
        0,
    );
    assert_eq!(
        run(
            &image,
            &page("", &format!("<noscript><p>x</p></noscript>{ROW}"))
        )
        .unwrap()
        .len(),
        1
    );
}

#[test]
fn benign_and_foreign_documents_continue() {
    for selector in ["div.error", "span.error", "div.error:not(:has(.ok))"] {
        let plan = guarded(selector);
        for body in [
            "<noscript></noscript>",
            "<template><b class='ok'>x</b></template>",
            "<template><template><p>nested</p></template></template>",
            "<svg><noscript><g></g></noscript></svg>",
            "<math><noscript><mi>x</mi></noscript></math>",
            "<svg><select><g></g></select></svg>",
            "<math><select><mi>x</mi></select></math>",
            "<svg><template shadowrootmode='open'><g></g></template></svg>",
            "<math><template shadowrootmode='closed'><mi>x</mi></template></math>",
            "<div data-shadowrootmode='open'>x</div>",
            "<option>loose</option>",
        ] {
            let releases = run(&plan, &page("", &format!("{body}{ROW}")))
                .unwrap_or_else(|e| panic!("{selector} {body}: {e}"));
            assert_eq!(releases.len(), 1, "{selector} {body}");
        }
    }
}

#[test]
fn ordinary_inert_templates_keep_upstream_structure() {
    // Template contents are outside ChildNodes: `.ok` there is invisible to `:has`.
    let has = guarded("div.error:not(:has(.ok))");
    source_error(
        run(
            &has,
            &page(
                "",
                "<div class='error'><template><b class='ok'>x</b></template>Login required</div>",
            ),
        ),
        0,
    );
    source_error(
        run(
            &has,
            &page(
                "",
                "<div class='error'><template><template><b class='ok'>x</b></template></template>Login required</div>",
            ),
        ),
        0,
    );
    let control = page(
        "",
        &format!("<div class='error'><b class='ok'>x</b>Login required</div>{ROW}"),
    );
    assert_eq!(run(&has, &control).unwrap().len(), 1);
    // A template with contents is still `:empty`; an element inside one is not selected.
    source_error(
        run(
            &guarded("template.x:empty"),
            &page("", "<template class='x'><b>content</b></template>"),
        ),
        0,
    );
    let nested = guarded("div.error");
    let inert = page(
        "",
        &format!("<template><template><div class='error'>x</div></template></template>{ROW}"),
    );
    assert_eq!(run(&nested, &inert).unwrap().len(), 1);
}

#[test]
fn plans_without_error_blocks_ignore_the_guard() {
    let plan = compile(&with_errors("52bt", "")).unwrap();
    for (body, _) in UNSUPPORTED {
        // Unrelated markup around the row does not change ordinary extraction.
        let body = page("", &format!("{ROW}{}", body.replace("class='error'", "")));
        assert_eq!(run(&plan, &body).unwrap().len(), 1, "{body}");
    }
}
