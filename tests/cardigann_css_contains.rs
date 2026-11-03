//! Synthetic fixtures for AngleSharp's CSS `:contains(...)` using unchanged
//! pinned definitions. Source: AngleSharp v1.8.6 (9253950e551f9ed95294f775675de9aa249894a2)
//! `CssSelectorConstructor.cs:25-43,376,958-1000` and
//! `TextContainsMatcher.cs:28-94`, `HtmlTemplateElement.cs:60-94`,
//! `HtmlDomBuilder.cs:2623-2640,3034-3059`; Jackett fe8876d2 `CardigannIndexer.cs:1277-1335,1795,1829`.
//! No live responses or credentials.
use chrono::{TimeZone, Utc};
use rackett::{
    cardigann::{parse, Limits},
    cardigann_exec::{
        css::{self, Css, Scan},
        selector::{self, Selector},
        template::Template,
        Plan,
    },
    models::SearchQuery,
};
use scraper::{ElementRef, Html};
use std::collections::BTreeMap;

fn source(name: &str) -> String {
    std::fs::read_to_string(format!("vendor/jackett/definitions/{name}.yml")).unwrap()
}
fn pinned(name: &str) -> Plan {
    Plan::compile(&parse(&source(name), Limits::default()).unwrap()).unwrap()
}
fn settings(values: &[(&str, bool)]) -> BTreeMap<String, serde_json::Value> {
    values
        .iter()
        .map(|(k, v)| ((*k).into(), serde_json::json!(v)))
        .collect()
}
/// Render the pinned row selector exactly as the engine does for the settings.
fn row_selector(name: &str, config: &[(&str, bool)]) -> String {
    let plan = pinned(name);
    let node = parse(&source(name), Limits::default()).unwrap();
    let raw = node
        .get("search")
        .and_then(|s| s.get("rows"))
        .and_then(|r| r.string("selector"))
        .unwrap()
        .to_owned();
    let context = plan.context(
        &settings(config),
        &SearchQuery::default(),
        Utc.with_ymd_and_hms(2026, 10, 27, 12, 0, 0).unwrap(),
    );
    Template::compile(&raw).unwrap().render(&context).unwrap()
}
fn row_texts(body: &str, selector: &str) -> Vec<String> {
    let html = Html::parse_document(body);
    selector::html_rows(&html, selector, 1000)
        .unwrap()
        .iter()
        .map(|row| row.attr("id").unwrap_or_default().to_owned())
        .collect()
}
fn field(source: &str, body: &str) -> Result<Option<String>, String> {
    let html = Html::parse_document(body);
    let row = selector::html_rows(&html, "article", 1).unwrap()[0];
    Selector::compile(&parse(source, Limits::default()).unwrap())
        .unwrap()
        .extract_html(row, &|s| Ok(s.into()))
}
fn matches(selector: &str, body: &str) -> bool {
    try_matches(selector, body).unwrap()
}
/// Engine parser (`css::parse_document`), first `#t` element.
fn try_matches(selector: &str, body: &str) -> Result<bool, String> {
    let html = css::parse_document(body);
    let target = select_first(&html, "#t");
    Css::parse(selector)?.matches(&target, &Scan::default())
}
fn select_first<'a>(html: &'a Html, s: &str) -> ElementRef<'a> {
    html.select(&scraper::Selector::parse(s).unwrap())
        .next()
        .unwrap()
}

#[test]
fn torrentbyte_rows_exclude_no_results_text_through_full_plan() {
    let plan = pinned("torrentbyte");
    let context = plan.context(
        &BTreeMap::new(),
        &SearchQuery::default(),
        Utc.with_ymd_and_hms(2026, 10, 27, 12, 0, 0).unwrap(),
    );
    let request = plan.build_requests(&context).unwrap().remove(0);
    let data = "<tr><td title='Release A'><a href='?view=1'>Release A</a><a href='magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567'>m</a></td><td>Movies</td><td>1.5 GB</td><td>2</td><td>2026-10-01 10:00:00</td><td>Fixture <a>drop</a></td></tr>";
    for empty in [
        "<tr><td colspan=6>No matching torrent records found.</td></tr>",
        // TextContent concatenates across node boundaries.
        "<tr><td colspan=6>No matching <b>torrent</b> records<!-- x --> found.</td></tr>",
    ] {
        let body = format!("<table><tbody>{empty}</tbody></table>");
        assert!(plan.extract(&request, &body, &context).unwrap().is_empty());
        let body = format!("<table><tbody>{data}{empty}</tbody></table>");
        let rows = plan.extract(&request, &body, &context).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Release A");
        assert_eq!(rows[0].size, Some(1_610_612_736));
        assert_eq!(rows[0].attributes["description"], "Fixture");
    }
    // Template contents are inert: the row is data, not a no-result row.
    let templated = data.replace(
        ">m</a></td>",
        ">m</a><template>No matching torrent records found.</template></td>",
    );
    let body = format!("<table><tbody>{templated}</tbody></table>");
    assert_eq!(plan.extract(&request, &body, &context).unwrap().len(), 1);
    // html5ever drops a declarative shadow root template and leaks its text
    // into the host; that DOM cannot match upstream, so extraction fails.
    let shadow = data.replace(
        ">m</a></td>",
        ">m</a><template shadowrootmode=open>No matching torrent records found.</template></td>",
    );
    let body = format!("<table><tbody>{shadow}</tbody></table>");
    let err = plan.extract(&request, &body, &context).unwrap_err();
    assert!(err.message.contains("declarative shadow root"), "{err:?}");
    // Comment data is not text, so this sentence no longer matches and the
    // unexcluded no-result row fails as a required-field row.
    let body =
        "<table><tbody><tr><td>No matching <!--torrent -->records found.</td></tr></tbody></table>";
    assert!(plan.extract(&request, body, &context).is_err());
}

#[test]
fn sportscult_split_recommendation_block_is_excluded() {
    let selector = row_selector("sportscult", &[]);
    let row = |id| {
        format!("<tr id={id}><td><a href='index.php?page=torrents&category=1'>x</a></td></tr>")
    };
    let body = format!(
        "<div><table class=lista><tr><td class=block>Our <b>Team</b> Recommend</td></tr>{}</table></div>\
         <div><table class=lista><tr><td class=block>Our  Team Recommend</td></tr>{}</table></div>\
         <div><table class=lista>{}{}</table></div>",
        row("rec"),
        row("spaced"),
        row("a"),
        row("b")
    );
    // Exact whitespace: a doubled space is not the recommendation block.
    assert_eq!(row_texts(&body, &selector), ["spaced", "a", "b"]);
}

#[test]
fn eztv_selector_list_keeps_document_order_without_duplicates() {
    let selector = row_selector("eztv", &[]);
    let row = |id| {
        format!("<tr id={id} name=hover class=forum_header_border><td><a class=magnet href='magnet:?'>m</a></td></tr>")
    };
    let body = format!(
        "<table class=forum_header_border><tr><td>Releases</td></tr>{}</table>\
         <table class=forum_header_border><tr><td>Latest Releases</td></tr>{}{}</table>\
         <table class=forum_header_border><tr><td>latest</td></tr>{}</table>\
         <table class=forum_header_border><tr><td>Lat<i>est</i></td></tr>{}<tr id=nomagnet name=hover class=forum_header_border></tr></table>",
        row("r1"),
        row("both1"),
        row("both2"),
        row("lowercase"),
        row("split")
    );
    assert_eq!(
        row_texts(&body, &selector),
        ["r1", "both1", "both2", "split"]
    );
}

#[test]
fn rgfootball_not_has_vip_branch() {
    let row = |id, vip: &str, gold: &str| {
        format!("<tr id=tor_{id}><td><a href='./download.php?id={id}'>d</a>{vip}{gold}</td></tr>")
    };
    let body = format!(
        "<table>{}{}{}{}</table>",
        row(1, "<a class=gen>VIP</a>", ""),
        row(2, "<a class=gen>vip</a>", "<img src='images/tor_gold.gif'>"),
        row(3, "<span>VIP</span>", ""),
        row(
            4,
            "<a class=gen>V<b>IP</b></a>",
            "<img src='images/tor_gold.gif'>"
        )
    );
    let ids = |config| row_texts(&body, &row_selector("rgfootball", config));
    assert_eq!(ids(&[]), ["tor_1", "tor_2", "tor_3", "tor_4"]);
    assert_eq!(ids(&[("ignorevip", true)]), ["tor_2", "tor_3"]);
    assert_eq!(ids(&[("ignorevip", true), ("freeleech", true)]), ["tor_2"]);
}

#[test]
fn punkshorror_nth_child_combination() {
    let row = |id, first: &str| {
        format!("<tr id={id}><td><a href='details.php?id={id}'>t</a></td><td><span>{first}</span><span>0</span></td><td>x</td><td>y</td><td>z</td></tr>")
    };
    let body = format!(
        "<table><tr><td><table><tr><td><table><tbody>{}{}{}</tbody></table></td></tr></table></td></tr></table>",
        row("free", "x0x"),
        row("paid", "1"),
        row("other", "")
    );
    let selector = row_selector("punkshorror", &[]);
    assert_eq!(row_texts(&body, &selector), ["free", "paid", "other"]);
    let selector = row_selector("punkshorror", &[("freeleech", true)]);
    assert!(selector.contains(":contains(\"0\")"));
    assert_eq!(row_texts(&body, &selector), ["free"]);
}

#[test]
fn text_content_semantics() {
    let script = "<div id=t><script>var token = 'abc';</script><style>.x{}</style></div>";
    assert!(matches(":contains(\"token = 'abc'\")", script));
    assert!(matches(":contains(\".x{}\")", script));
    // Comments are excluded and do not break adjacent text.
    let comment = "<div id=t>ab<!--hidden-->cd</div>";
    assert!(matches(":contains(abcd)", comment));
    assert!(!matches(":contains(hidden)", comment));
    // The empty needle matches without scanning, including empty elements.
    assert!(matches(":contains(\"\")", "<p id=t></p>"));
    assert!(matches(":contains('')", "<p id=t></p>"));
    assert!(matches(":contains(\"\")", "<p id=t></p>"));
    // Function name is ASCII case-insensitive; text is ordinal.
    assert!(matches(":CoNtAiNs(Free)", "<p id=t>Free</p>"));
    assert!(!matches(":contains(free)", "<p id=t>Free</p>"));
    assert!(!matches(":contains(FREE)", "<p id=t>Free</p>"));
    // Exact whitespace and surrounding argument whitespace.
    assert!(matches(":contains( \"a  b\" )", "<p id=t>xa  by</p>"));
    assert!(!matches(":contains(\"a  b\")", "<p id=t>a b</p>"));
    assert!(!matches(":contains(\"a b\")", "<p id=t>a\nb</p>"));
    // Attributes are not text.
    assert!(!matches(":contains(secret)", "<p id=t title=secret>x</p>"));
    // KMP restarts correctly across node boundaries.
    assert!(matches(":contains(aab)", "<p id=t>a<b>a</b>a<i>b</i></p>"));
    assert!(matches(
        ":contains(\"日本語\")",
        "<p id=t>日<b>本</b>語</p>"
    ));
}

#[test]
fn escaped_identifiers_follow_css_escape_decoding() {
    let nbsp = "<p id=t>1.5\u{a0}GB</p>";
    let space = "<p id=t>1.5 GB</p>";
    assert!(matches(":contains(\\00a0GB)", nbsp));
    assert!(!matches(":contains(\\00a0GB)", space));
    assert!(matches(":contains(\\a0 GB)", nbsp)); // One trailing space ends the escape.
    assert!(matches(":contains(\" GB\")", space));
    // `B` is a hex digit: `\00a0B` is U+0A0B, not NBSP followed by `B`.
    assert!(!matches(":contains(\\00a0B)", "<p id=t>1\u{a0}B</p>"));
    assert!(matches(":contains(\\00a0B)", "<p id=t>1\u{a0b}</p>"));
    // Unchanged pinned rutor size field: GB wins in source order of cases' td.
    let rutor = source("rutor");
    let size = rutor
        .lines()
        .find(|l| l.contains("td:contains(\\00a0TB)"))
        .unwrap()
        .trim();
    let source = format!("{size}\n");
    assert_eq!(
        field(
            &source,
            "<article><table><tr><td>a</td><td>1.2\u{a0}GB</td><td>3</td></tr></table></article>"
        )
        .unwrap()
        .as_deref(),
        // Field text is trimmed only (pinned ParseUtil.NormalizeSpace is Trim), so the
        // non-breaking space inside the value is kept.
        Some("1.2\u{a0}GB")
    );
    assert_eq!(
        field(
            &source,
            "<article><table><tr><td>a</td><td>12 B</td></tr></table></article>"
        )
        .unwrap(),
        None
    );
}

#[test]
fn cases_removal_and_not_has_semantics() {
    let body = "<article><span class=badge>Free <b>Leech</b></span><em>bad</em> text</article>";
    assert_eq!(
        field(
            "case:\n  'span:contains(\"Free Leech\")': yes\n  '*': no",
            body
        )
        .unwrap(),
        Some("yes".into())
    );
    assert_eq!(
        field(
            "case:\n  'span:contains(\"Free  Leech\")': yes\n  '*': no",
            body
        )
        .unwrap(),
        Some("no".into())
    );
    assert_eq!(
        field(
            "remove: 'em:contains(bad), span:not(:contains(Free))'",
            body
        )
        .unwrap(),
        Some("Free Leech text".into())
    );
    assert_eq!(
        field("selector: ':not(:has(:contains(Leech)))'\nremove: em", body).unwrap(),
        Some("Leech".into()) // First descendant: <b> has no descendants.
    );
    assert_eq!(
        field("selector: 'article:not(:has(:contains(Leech)))'", body).unwrap(),
        None
    );
    assert_eq!(
        field(
            "selector: 'article:not(:has(:contains(Missing)))'\nremove: 'em:contains(bad)'",
            body
        )
        .unwrap(),
        Some("Free Leech text".into())
    );
    // The row itself is checked before its descendants, as handleSelector's Matches.
    assert_eq!(
        field(
            "selector: ':contains(text)'\nattribute: id",
            "<article id=row>text</article>"
        )
        .unwrap(),
        Some("row".into())
    );
}

#[test]
fn grammar_rejections_are_explicit() {
    for bad in [
        "p:contains()",
        "p:contains( )",
        "p:contains(1)",
        "p:contains(1.5)",
        "p:contains(a,b)",
        "p:contains(,)",
        "p:contains(.)",
        "p:contains(-)",
        "p:contains(\"abc",
        "p:contains('abc",
        "p:contains(\"abc\\\"",
        "p:contains(\"abc\\",
        "p:contains(\"abc\"",
        "p:contains(/*c*/\"abc)",
        "p:contains(/*c*/)",
        "p:contains(abc",
        "p:contains(\"a\nb\")",
        "p:contains(a(b))",
        "p:contains",
        "p:is(:contains(1))",
        "p:not(:contains())",
    ] {
        let err = Css::parse(bad).unwrap_err();
        assert!(err.contains("unsupported CSS selector"), "{bad}: {err}");
    }
    for good in [
        "p:contains(a)",
        "p:contains( a )",
        "p:contains(\"a b\")",
        "p:contains('a)b')",
        "p:contains(\"a\\\"b\")",
        "p:contains(\"a\\\\\")",
        "p:contains(-a)",
        "p:contains(\"\")",
        "p:contains(/* c */a/* d */)",
        "p:contains(/*c*/\"a\")",
        "p:contains( /*c*/ 'a)' /*d*/ )",
        "p:CONTAINS(a)",
        "p:is(:contains(a), b)",
        "p:contains(a b)",
        "p:contains(\"a\" \"b\")",
        "p:contains(a \"b\")",
        "p:contains(/*c*/ a /*d*/ b)",
        "p:contains(/*c*/\"a\"/*d*/\"b\")",
        "p:where(:contains(a b), p)",
        "p:has(:contains(\"x\" y))",
    ] {
        assert!(Css::parse(good).is_ok(), "{good}");
    }
    // Other unsupported selectors keep scraper's diagnostics verbatim.
    for bad in ["a[x=y z]", ":hover", "::before", ":foo(x)", "a >", ""] {
        let expected = format!(
            "unsupported CSS selector {bad:?}: {:?}",
            scraper::Selector::parse(bad).unwrap_err()
        );
        assert_eq!(Css::parse(bad).unwrap_err(), expected);
    }
}

#[test]
fn builtin_selectors_match_scraper_exactly() {
    let html = Html::parse_document(
        "<html><body><div id=a class='x y' lang=en-US><p>one</p><span data-k='v w'>two</span>\
         <p class=X>three<a href='/q'>l</a></p><!-- c --><p></p></div><ul><li>1<li>2<li>3<li>4</ul>\
         <svg><circle/></svg><table><tr><td>a<td>b</table><template><b>t</b></template></body></html>",
    );
    let selectors = [
        "*",
        ":root",
        ":scope",
        ":scope > body",
        "div:has(> span)",
        "div:has(a[href^='/'])",
        "p:not(:has(a))",
        ":is(p, span):not(.X)",
        ":where(li):nth-child(2n+1)",
        "li:nth-last-child(2)",
        "p:nth-of-type(2)",
        "p:nth-last-of-type(1)",
        "p:first-child, li:last-child",
        "p:only-child, b:only-of-type",
        "p:empty",
        "p ~ span, p + p",
        "[data-k~=w]",
        "[lang|=en]",
        "[class~=x i]",
        ".x.y#a",
        "*|circle",
        "|circle",
        "td:first-of-type",
        "div > p:has(+ p)",
        "template b",
        "div :is(p:has(a), span)",
    ];
    let nodes = html
        .tree
        .nodes()
        .filter_map(ElementRef::wrap)
        .collect::<Vec<_>>();
    for s in selectors {
        let expected = scraper::Selector::parse(s).unwrap();
        let actual = Css::parse(s).unwrap();
        let scan = Scan::default();
        for node in &nodes {
            assert_eq!(
                actual.matches(node, &scan).unwrap(),
                expected.matches(node),
                "{s} on {:?}",
                node.value()
            );
        }
    }
}

#[test]
fn scan_budget_exhaustion_is_an_error_not_a_non_match() {
    let html = Html::parse_document("<div id=t><p>abc</p><p>def</p></div>");
    let target = select_first(&html, "#t");
    // div, p, text, p, text: 5 nodes plus 6 text bytes.
    for (selector, expected) in [
        (":contains(def)", true),
        (":contains(xyz)", false),
        (":not(:contains(xyz))", true),
        (":has(p:contains(xyz))", false),
    ] {
        let css = Css::parse(selector).unwrap();
        assert_eq!(
            css.matches(&target, &Scan::with_limit(64)).unwrap(),
            expected
        );
        let err = css.matches(&target, &Scan::with_limit(5)).unwrap_err();
        assert!(err.contains("scan budget"), "{selector}: {err}");
    }
    // Exhaustion is sticky for the rest of the pass.
    let scan = Scan::with_limit(10);
    assert!(Css::parse(":contains(xyz)")
        .unwrap()
        .matches(&target, &scan)
        .is_err());
    assert!(Css::parse("div").unwrap().matches(&target, &scan).is_err());
    // Exact budget: 11 units exactly suffice for a full failed scan.
    assert!(!Css::parse(":contains(xyz)")
        .unwrap()
        .matches(&target, &Scan::with_limit(11))
        .unwrap());
    // Empty needles scan nothing.
    assert!(Css::parse(":contains('')")
        .unwrap()
        .matches(&target, &Scan::with_limit(0))
        .unwrap());

    // Real 32 MiB pass budget: 100 nested sections over 512 KiB of text.
    let text = "x".repeat(512 * 1024);
    let body = format!(
        "<article>{}{text}{}</article>",
        "<section>".repeat(100),
        "</section>".repeat(100)
    );
    let html = Html::parse_document(&body);
    for rows in [
        "section:contains(zzz)",
        "section:not(:contains(zzz))",
        "article:has(section:contains(zzz))",
        "article:not(:has(section:contains(zzz)))",
    ] {
        let err = selector::html_rows(&html, rows, 1000).unwrap_err();
        assert!(err.contains("scan budget"), "{rows}: {err}");
    }
    assert_eq!(
        selector::html_rows(&html, "section:contains(x)", 1000)
            .unwrap()
            .len(),
        100
    );
    for source in [
        "case:\n  'section:contains(zzz)': yes\n  '*': no",
        "selector: 'section:contains(zzz)'",
        "remove: 'section:contains(zzz)'",
    ] {
        let err = field(source, &body).unwrap_err();
        assert!(err.contains("scan budget"), "{source}: {err}");
    }
    // A field selection stops at its first match, as AngleSharp's QuerySelector does:
    // the outermost section matches after one 512 KiB scan, inside the budget.
    assert_eq!(
        field(
            "case:\n  'section:not(:contains(zzz))': yes\n  '*': no",
            &body
        ),
        Ok(Some("yes".into()))
    );
    let mut document = Html::parse_document(&body);
    let row = selector::html_row_slots(&document, "article", 1).unwrap()[0];
    let remove =
        Selector::compile(&parse("remove: 'section:contains(zzz)'", Limits::default()).unwrap())
            .unwrap();
    let err = remove
        .extract_html_mut(&mut document, row, &mut 20_000_000, &|s| Ok(s.into()))
        .unwrap_err();
    assert!(err.contains("scan budget"), "{err}");
}

#[test]
fn html_template_contents_are_outside_descendant_text() {
    let body = "<div id=t>a<template>secret<p>c</p></template>b</div>";
    assert!(!matches(":contains(secret)", body));
    assert!(!matches(":contains(c)", body));
    // Text before and after the inert subtree concatenates, KMP state included.
    assert!(matches(":contains(ab)", body));
    assert!(!matches(":contains(asecret)", body));
    assert!(!matches(":has(:contains(secret))", body));
    assert!(matches(":has(template)", body));
    // Template element itself: no child text, but the empty needle matches.
    let html = css::parse_document(body);
    let template = select_first(&html, "template");
    for (selector, expected) in [
        (":contains(secret)", false),
        (":contains(c)", false),
        (":contains('')", true),
        (":not(:contains(secret))", true),
    ] {
        let css = Css::parse(selector).unwrap();
        assert_eq!(
            css.matches(&template, &Scan::default()).unwrap(),
            expected,
            "{selector}"
        );
    }
    // Nested templates, including an unclosed one closed at end of input.
    let nested = "<div id=t>x<template>o<template>inner</template>p</template>y</div>";
    assert!(matches(":contains(xy)", nested));
    assert!(!matches(":contains(inner)", nested));
    assert!(!matches(":contains(op)", nested));
    assert!(!matches(
        ":contains(unclosed)",
        "<div id=t>a<template>unclosed"
    ));
    // Elements inside template contents keep normal text below themselves,
    // excluding their own nested template contents.
    let content = css::parse_document(
        "<div>x<template><section>o<b>k</b><template>hidden</template>!</section></template></div>",
    );
    let section = content
        .tree
        .nodes()
        .filter_map(ElementRef::wrap)
        .find(|e| e.value().name() == "section")
        .unwrap();
    let scan = Scan::default();
    assert!(Css::parse(":contains(\"ok!\")")
        .unwrap()
        .matches(&section, &scan)
        .unwrap());
    assert!(!Css::parse(":contains(hidden)")
        .unwrap()
        .matches(&section, &scan)
        .unwrap());
    // Head and table templates parse into inert contents as well.
    assert!(!matches(
        ":contains(head)",
        "<html id=t><head><template>head</template></head><body>x</body></html>"
    ));
    let table = "<table id=t><template><tr><td>t</td></tr></template><tr><td>r</td></tr></table>";
    assert!(matches(":contains(r)", table));
    assert!(!matches(":contains(t)", table));
    // Ordinary text, script and style are unchanged.
    assert!(matches(
        ":contains(\"ab;c\")",
        "<div id=t>a<span>b</span><script>;</script><style>c</style></div>"
    ));
}

#[test]
fn foreign_elements_named_template_keep_their_text() {
    let svg = "<div id=t><svg><template>svg<text>t</text></template></svg>x</div>";
    assert!(matches(":contains(svgtx)", svg));
    let html = css::parse_document(svg);
    let template = select_first(&html, "template");
    assert_eq!(&*template.value().name.ns, "http://www.w3.org/2000/svg");
    assert!(Css::parse(":contains(svgt)")
        .unwrap()
        .matches(&template, &Scan::default())
        .unwrap());
    let math = "<div id=t><math><template>mathml</template></math>!</div>";
    assert!(matches(":contains(\"mathml!\")", math));
    let html = css::parse_document(math);
    assert_eq!(
        &*select_first(&html, "template").value().name.ns,
        "http://www.w3.org/1998/Math/MathML"
    );
    // An HTML template nested in foreign content via an integration point.
    let mixed =
        "<div id=t><svg><foreignObject><template>inert</template>live</foreignObject></svg></div>";
    assert!(matches(":contains(live)", mixed));
    assert!(!matches(":contains(inert)", mixed));
}

#[test]
fn template_contents_are_not_charged_and_budget_errors_stay_explicit() {
    // div, "ab", template, "cd": 4 nodes plus 4 bytes; contents are skipped.
    let html = css::parse_document("<div id=t>ab<template>xxxxxxxxxx<b>y</b></template>cd</div>");
    let target = select_first(&html, "#t");
    let css = Css::parse(":contains(zz)").unwrap();
    assert!(!css.matches(&target, &Scan::with_limit(8)).unwrap());
    let err = css.matches(&target, &Scan::with_limit(7)).unwrap_err();
    assert!(err.contains("scan budget"), "{err}");
    let err = Css::parse(":not(:contains(zz))")
        .unwrap()
        .matches(&target, &Scan::with_limit(7))
        .unwrap_err();
    assert!(err.contains("scan budget"), "{err}");
}

#[test]
fn dropped_declarative_shadow_roots_fail_explicitly() {
    // html5ever 0.29 drops `open`/`close` (sic) templates and leaks their
    // contents into the host; AngleSharp attaches an open shadow root.
    for mode in ["open", "close"] {
        let body = format!(
            "<div id=t>a<template shadowrootmode={mode}><p>secret</p></template>visible</div>"
        );
        let plain = Html::parse_document(&body);
        assert!(plain
            .select(&scraper::Selector::parse("template").unwrap())
            .next()
            .is_none());
        for selector in [
            ":contains(secret)",
            ":contains(visible)",
            ":not(:contains(x))",
            "div:contains(secret)",
        ] {
            let err = try_matches(selector, &body).unwrap_err();
            assert!(err.contains("declarative shadow root"), "{selector}: {err}");
        }
        // No traversal: empty needles and ordinary selectors are unaffected.
        assert!(try_matches(":contains('')", &body).unwrap());
        assert!(try_matches("div", &body).unwrap());
        // Rejected per selector even where matching would fail on `p` first.
        let err = try_matches("p:contains(secret)", &body).unwrap_err();
        assert!(err.contains("declarative shadow root"), "{err}");
        // The marker sits before the root element and leaves its tree intact.
        let marked = css::parse_document(&body);
        assert_eq!(marked.root_element().html(), plain.root_element().html());
        for rows in ["div:contains(visible)", "p:contains(secret)"] {
            let err = selector::html_rows(&marked, rows, 10).unwrap_err();
            assert!(err.contains("declarative shadow root"), "{rows}: {err}");
        }
    }
    // Other modes keep an ordinary template in both parsers' text model:
    // AngleSharp attaches `closed`/`Open` shadow roots, also outside ChildNodes.
    for mode in ["closed", "Open", "none"] {
        let body = format!(
            "<div id=t>a<template shadowrootmode={mode}><p>secret</p></template>visible</div>"
        );
        assert!(!matches(":contains(secret)", &body), "{mode}");
        assert!(matches(":contains(avisible)", &body), "{mode}");
    }
}

#[test]
fn engine_parser_matches_scraper_for_ordinary_documents() {
    for body in [
        "<!doctype html><title>x</title><div>a<template>b<p>c</p></template>d</div>",
        "<table><template><tr><td>t</td></tr></template><tr><td>r</td></tr></table>",
        "<div><svg><template>s</template></svg><math><mi>m</mi></math></div>",
        "<p>unclosed <b>formatting <i>nesting</p> text<form><input></form>",
        "<div>a<template shadowrootmode=closed>b</template></div>",
    ] {
        assert_eq!(
            css::parse_document(body).html(),
            Html::parse_document(body).html(),
            "{body}"
        );
    }
}

#[test]
fn several_argument_tokens_keep_the_last_like_anglesharp() {
    // ContainsFunctionState (AngleSharp 1.8.6, CssSelectorConstructor.cs:958-998) overwrites
    // its value with every ident or string token; the tokenizer drops comments.
    assert!(matches(":contains(Only Upload)", "<p id=t>Upload</p>"));
    assert!(!matches(":contains(Only Upload)", "<p id=t>Only</p>"));
    assert!(matches(":contains(\"x y\" z)", "<p id=t>z</p>"));
    assert!(matches(":contains(a/*c*/'b c')", "<p id=t>b c</p>"));
    assert!(!matches(":contains(a/*c*/'b c')", "<p id=t>a</p>"));
}

#[test]
fn comments_before_quoted_argument_use_token_boundaries() {
    assert!(matches(":contains(/*c*/\"a b\")", "<p id=t>xa by</p>"));
    assert!(!matches(":contains(/*c*/'a  b')", "<p id=t>a b</p>"));
}

/// Raw scraper parse versus engine parse of the same document.
fn plain_matches(selector: &str, body: &str) -> bool {
    let html = Html::parse_document(body);
    Css::parse(selector)
        .unwrap()
        .matches(&select_first(&html, "#t"), &Scan::default())
        .unwrap()
}
#[test]
fn html_noscript_content_is_parsed_as_markup_like_upstream() {
    // Jackett's `new HtmlParser()` disables scripting (`HtmlParser.cs:64-67,83`).
    // Former false matches: scripting-enabled raw text exposes markup as text.
    let image = "<div id=t><noscript><img alt=\"Freeleech\"></noscript></div>";
    assert!(plain_matches(":contains(Freeleech)", image));
    assert!(plain_matches(":contains(\"<\")", image));
    assert!(!matches(":contains(Freeleech)", image));
    assert!(!matches(":contains(\"<\")", image));
    let html = css::parse_document(image);
    assert_eq!(
        select_first(&html, "noscript > img").attr("alt"),
        Some("Freeleech")
    );
    // Former missed match: the reference is decoded as markup.
    let entity = "<div id=t><noscript>Tom &amp; Jerry</noscript></div>";
    assert!(!plain_matches(":contains(\"Tom & Jerry\")", entity));
    assert!(matches(":contains(\"Tom & Jerry\")", entity));
    assert!(!matches(":contains(\"&amp;\")", entity));
    // Head placement: in-head-noscript mode closes noscript and head at text,
    // leaving an empty noscript in head and moving the text into body
    // (`HtmlDomBuilder.cs:797-812,912-970`).
    let head = "<html><head><noscript>Freeleech</noscript></head><body id=t><p>x</p></body></html>";
    assert!(!plain_matches(":contains(Freeleech)", head));
    assert!(matches(":contains(Freeleech)", head));
    assert!(matches(":contains(Freeleechx)", head));
    let html = css::parse_document(head);
    assert!(!select_first(&html, "head > noscript").has_children());
    // Head content stays in head noscript.
    let link =
        "<html><head><noscript><link id=l rel=x></noscript></head><body id=t>ok</body></html>";
    let html = css::parse_document(link);
    assert_eq!(
        select_first(&html, "head > noscript > link").attr("id"),
        Some("l")
    );
    assert!(matches(":contains(ok)", link));
    // Elements only this parse creates are selected.
    let element = "<article id=t><noscript><b>Free</b></noscript></article>";
    assert!(matches(":has(b:contains(Free))", element));
    assert!(!matches(":not(:has(b:contains(Free)))", element));
    let html = css::parse_document(element);
    assert_eq!(
        select_first(&html, "noscript b").text().collect::<String>(),
        "Free"
    );
    // Nested noscript content inside inert template contents is still inert.
    let template = "<div id=t>a<template><noscript><b>x</b></noscript></template>b</div>";
    assert!(matches(":contains(ab)", template));
    assert!(!matches(":contains(x)", template));
    // Empty HTML noscript and foreign noscript elements are ordinary.
    assert!(matches(
        ":contains(ok)",
        "<div id=t><noscript></noscript>ok</div>"
    ));
    let svg = "<div id=t><svg><noscript>vec<b>tor</b></noscript></svg></div>";
    assert!(matches(":contains(vector)", svg));
    let html = css::parse_document(svg);
    assert_eq!(
        &*select_first(&html, "noscript").value().name.ns,
        "http://www.w3.org/2000/svg"
    );
    // Script and style raw text and their ancestors' text are unchanged.
    let script = "<div id=t><script>if (a < b) x = '&amp;';</script><style>p>b{}</style></div>";
    assert!(matches(":contains(\"a < b\")", script));
    assert!(matches(":contains(\"&amp;\")", script));
    assert!(matches(":contains(\"p>b\")", script));
    assert!(plain_matches(":contains(\"&amp;';p>b\")", script));
    assert!(matches(":contains(\"&amp;';p>b\")", script));
}

/// Synthetic plan: `article` rows and an optional freeleech case field whose
/// default would silently claim a full download factor if errors were swallowed.
fn optional_case_plan(rows: &str, case_key: &str) -> Plan {
    let source = format!(
        "id: contains\nname: Contains\ndescription: Synthetic\nlanguage: en-US\ntype: public\nencoding: UTF-8\nlinks: [https://example.org/]\ncaps:\n  categories: {{Other: Other}}\n  modes: {{search: [q]}}\nsearch:\n  paths: [{{path: search}}]\n  rows: {{selector: {}}}\n  fields:\n    category: {{text: Other}}\n    title: {{text: Fixture}}\n    size: {{text: 0}}\n    seeders: {{text: 0}}\n    date: {{text: now}}\n    download: {{text: /download}}\n    downloadvolumefactor: {{optional: true, default: \"1\", case: {{{}: \"0\"}}}}\n    description: {{optional: true, selector: i.missing}}\n",
        serde_json::to_string(rows).unwrap(),
        serde_json::to_string(case_key).unwrap(),
    );
    Plan::compile(&parse(&source, Limits::default()).unwrap()).unwrap()
}
fn run(
    plan: &Plan,
    body: &str,
) -> Result<Vec<rackett::models::Release>, rackett::cardigann_exec::ExecutionError> {
    let context = plan.context(
        &BTreeMap::new(),
        &SearchQuery::default(),
        Utc.with_ymd_and_hms(2026, 10, 27, 12, 0, 0).unwrap(),
    );
    let request = plan.build_requests(&context).unwrap().remove(0);
    plan.extract(&request, body, &context)
}

#[test]
fn optional_fields_never_swallow_contains_failures() {
    let plan = optional_case_plan("article", "b:contains(Free)");
    // Ordinary behavior: case match, default and optional missing selector.
    let free = run(&plan, "<article><b>Free</b></article>").unwrap();
    assert_eq!(free[0].attributes["downloadvolumefactor"], "0");
    assert!(!free[0].attributes.contains_key("description"));
    let paid = run(&plan, "<article><b>Paid</b></article>").unwrap();
    assert_eq!(paid[0].attributes["downloadvolumefactor"], "1");
    // Noscript content is markup, as upstream: the case finds <b>Free</b>.
    let noscript = run(&plan, "<article><noscript><b>Free</b></noscript></article>").unwrap();
    assert_eq!(noscript[0].attributes["downloadvolumefactor"], "0");
    let plan = optional_case_plan("article:not(:contains(zzz))", "b:contains(Free)");
    let noscript = run(&plan, "<article><noscript>x</noscript></article>").unwrap();
    assert_eq!(noscript[0].attributes["downloadvolumefactor"], "1");

    let text = "x".repeat(512 * 1024);
    let budget = format!(
        "<article>{}{text}{}</article>",
        "<section>".repeat(100),
        "</section>".repeat(100)
    );
    let cases = [
        (
            optional_case_plan("article", "section:contains(zzz)"),
            budget,
            "scan budget",
        ),
        (
            optional_case_plan("article", "b:contains(Free)"),
            "<article><template shadowrootmode=open><b>Free</b></template></article>".into(),
            "declarative shadow root",
        ),
    ];
    for (plan, body, reason) in cases {
        let err = run(&plan, &body).unwrap_err();
        assert_eq!(err.path, "$.response", "{reason}");
        assert!(err.message.starts_with("CSS :contains "), "{err:?}");
        assert!(err.message.contains(reason), "{err:?}");
    }
}

#[test]
fn a_repeated_compound_type_is_a_no_op_like_anglesharp() {
    // AngleSharp adds a type selector anywhere in a compound to that compound
    // (CssSelectorConstructor.cs:211-213,465-492): `a:not(x)a` is `a:not(x)`.
    let link = "<p><a id=t href=/x>1080p BluRay</a></p>";
    assert!(matches("a:not(:contains(HDTV))a:contains(\"1080\")", link));
    assert!(!matches(
        "a:not(:contains(BluRay))a:contains(\"1080\")",
        link
    ));
    assert!(matches("p A:not(.x)a", link));
    // Other misplaced type selectors stay rejected.
    for bad in [":not(.x)a", "a:not(.x)b", "a:not(.x)ns|a", "p :not(.x)a"] {
        assert!(Css::parse(bad).is_err(), "{bad}");
    }
    // Pseudo-class names, classes and argument contents are never treated as types.
    assert!(Css::parse("a:first-child.a:not(a)").is_ok());
    let pinned = r#"td:nth-child(2) a:not(:contains("HDTV"))a:not(:contains("hdtv"))a:not(:contains("REMUX"))a:not(:contains("Remux"))a:not(:contains("remux"))a:not(:contains("WEB"))a:not(:contains("web"))a:not(:contains("Web"))a:contains("1080"),:contains("2160"):contains("uhd")"#;
    let row = "<table><tr><td>x</td><td><a id=t>Film 1080p BluRay</a></td></tr></table>";
    assert!(matches(pinned, row));
    assert!(!matches(pinned, &row.replace("BluRay", "WEB-DL")));
}
