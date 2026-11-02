//! AngleSharp's CSS attribute inequality `[name!=value]` against unchanged
//! pinned definitions and synthetic fixtures. Source: AngleSharp v1.8.6
//! (9253950e551f9ed95294f775675de9aa249894a2) `CssTokenizer.cs:314-323`,
//! `CssSelectorConstructor.cs:252-360`, `DefaultAttributeSelectorFactory.cs:14-72,128-168`,
//! `AttrNotMatchSelector.cs:21`, `BaseAttrSelector.cs:15-28`, `Element.cs:442-450`;
//! Jackett fe8876d2 `carpathians.yml:96`, `3dtorrents.yml:112`.
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
fn titles(name: &str, config: &[(&str, bool)], body: &str) -> Vec<String> {
    let plan = pinned(name);
    let context = plan.context(
        &settings(config),
        &SearchQuery::default(),
        Utc.with_ymd_and_hms(2026, 10, 27, 12, 0, 0).unwrap(),
    );
    let request = plan.build_requests(&context).unwrap().remove(0);
    plan.extract(&request, body, &context)
        .unwrap()
        .into_iter()
        .map(|r| r.title)
        .collect()
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
fn rejected(selector: &str) -> String {
    let err = Css::parse(selector).unwrap_err();
    assert!(
        err.starts_with(&format!(
            "unsupported CSS selector {selector:?}: invalid [attribute!=value]: "
        )),
        "{selector}: {err}"
    );
    err
}

fn carpathians_row(id: &str, title: &str) -> String {
    format!(
        "<tr id={id}><td><a href='browse.php?cat=199'>c</a></td>\
         <td><a id=a{id} href='details.php?id={id}' title='{title}'>{title}</a>\
         <a href='download.php/{id}/x.torrent'>d</a>\
         <font class=line>Feltöltve: 2 napja | Akció</font></td>\
         <td></td><td>1</td><td></td><td>1.5 GB</td><td>3</td><td>10</td><td>2</td></tr>"
    )
}

#[test]
fn carpathians_excludes_only_the_exact_recommendation_table() {
    let selector = row_selector("carpathians", &[]);
    assert_eq!(selector, r#"table[id!="torrent_ajanlo"] > tbody > tr[id]"#);
    let table = |attrs: &str, id: &str, title: &str| {
        format!(
            "<table {attrs}><tbody>{}<tr><td>header</td></tr></tbody></table>",
            carpathians_row(id, title)
        )
    };
    let body = [
        table("id=torrent_ajanlo", "r0", "Recommended"),
        // Missing attribute: AngleSharp compares null, which is unequal.
        table("", "r1", "Listed One"),
        // Ordinal: neither case nor surrounding text equals the value.
        table("id=TORRENT_AJANLO", "r2", "Upper Case"),
        table("id='torrent_ajanlo '", "r3", "Trailing Space"),
        table("id=''", "r4", "Empty"),
    ]
    .concat();
    assert_eq!(
        titles("carpathians", &[], &body),
        ["Listed One", "Upper Case", "Trailing Space", "Empty"]
    );
    // Nested tables: the inner recommendation table is still excluded.
    let nested = format!(
        "<table><tbody><tr><td>{}</td></tr></tbody></table>",
        table("id=torrent_ajanlo", "r5", "Inner")
    );
    assert!(titles("carpathians", &[], &nested).is_empty());
}

fn three_d_row(id: u32, title: &str, free: bool) -> String {
    let sign = if free {
        "<img src='images/sign_free.png'>"
    } else {
        ""
    };
    format!(
        "<tr><td><a href='index.php?page=torrents&amp;category=14'>c</a></td>\
         <td><a href='index.php?page=torrent-details&amp;id={id}' \
         onmouseover=\"return overlib(',\\'http://p.example/{id}.jpg\\'')\">{title}</a>{sign}\
         <span>Drama</span></td><td>27/10/2026</td><td>2.0 GB</td>\
         <td><a href='index.php?page=peers&amp;id={id}'>5</a></td><td>1</td>\
         <td><a href='index.php?page=torrent_history&amp;id={id}'>9</a></td></tr>"
    )
}

#[test]
fn three_d_torrents_excludes_cellspacing_one_with_both_freeleech_branches() {
    assert_eq!(
        row_selector("3dtorrents", &[("freeleech", false)]),
        r#"table[cellspacing!="1"].lista > tbody > tr:has(a[href^="index.php?page=torrents&category="])"#
    );
    let free = row_selector("3dtorrents", &[("freeleech", true)]);
    assert!(free.ends_with(r#":has(img[src="images/sign_free.png"])"#));
    Css::parse(&free).unwrap();
    let table = |attrs: &str, rows: &str| {
        format!("<table class=lista {attrs}><tbody><tr><td>head</td></tr>{rows}</tbody></table>")
    };
    let body = [
        table("cellspacing=1", &three_d_row(1, "Layout", false)),
        table("cellspacing=0", &three_d_row(2, "Zero", false)),
        table("", &three_d_row(3, "Missing", true)),
        // The value is a string token, so `01` is a different value.
        table("cellspacing=01", &three_d_row(4, "Leading Zero", true)),
        // `.lista` still applies.
        "<table cellspacing=0><tbody>".to_owned() + &three_d_row(5, "No Class", true),
        "</tbody></table>".to_owned(),
    ]
    .concat();
    assert_eq!(
        titles("3dtorrents", &[("freeleech", false)], &body),
        ["Zero", "Missing", "Leading Zero"]
    );
    assert_eq!(
        titles("3dtorrents", &[("freeleech", true)], &body),
        ["Missing", "Leading Zero"]
    );
}

#[test]
fn inequality_equals_not_case_sensitive_equality() {
    let elements = [
        "<div id=t a=v></div>",
        "<div id=t a=V></div>",
        "<div id=t a=''></div>",
        "<div id=t></div>",
        "<div id=t a='v '></div>",
        "<div id=t A=v></div>",
        "<div id=t data-a=v></div>",
        "<div id=t a='x]y'></div>",
        "<div id=t a='\"'></div>",
        "<div id=t a='1'></div>",
    ];
    for (ours, reference) in [
        ("[a!=v]", r#":not([a="v" s])"#),
        ("[a!='v']", r#":not([a="v" s])"#),
        (r#"[a!="v"]"#, r#":not([a="v" s])"#),
        ("[ a != v ]", r#":not([a="v" s])"#),
        ("[a/*c*/!=/*d*/v/*e*/]", r#":not([a="v" s])"#),
        ("[a!=v s]", r#":not([a="v" s])"#),
        (r#"[a!="v"S]"#, r#":not([a="v" s])"#),
        (r#"[a!=""]"#, r#":not([a="" s])"#),
        (r#"[a!="x]y"]"#, r#":not([a="x]y" s])"#),
        (r#"[a!="\""]"#, r#":not([a='"' s])"#),
        (r#"[a!=x\]y]"#, r#":not([a="x]y" s])"#),
        (r"[a!=\76]", r#":not([a="v" s])"#),
        (r"[a!=\31]", r#":not([a="1" s])"#),
        ("[a!=1]", r#":not([a="1" s])"#),
        (r"[\61!=v]", r#":not([a="v" s])"#),
        ("[A!=v]", r#":not([a="v" s])"#),
        ("div[a!=v]", r#"div:not([a="v" s])"#),
        ("div[a!=v][a!=V]", r#"div:not([a="v" s]):not([a="V" s])"#),
    ] {
        for body in elements {
            let html = Html::parse_document(body);
            let expected = scraper::Selector::parse(reference)
                .unwrap()
                .matches(&select_first(&html, "#t"));
            assert_eq!(matches(ours, body), expected, "{ours} on {body}");
        }
    }
    // Missing attribute and empty value are distinct.
    assert!(matches("[a!='']", "<div id=t></div>"));
    assert!(!matches("[a!='']", "<div id=t a></div>"));
    assert!(matches("[a!=v]", "<div id=t></div>"));
}

#[test]
fn strings_comments_escapes_and_contains_arguments_are_not_rewritten() {
    let body = "<div id=t title='a!=b' class=c>[a!=b] x!=y</div>";
    assert!(matches(r#"[title="a!=b"]"#, body));
    assert!(matches(r#"[title='a!=b']"#, body));
    assert!(matches("/* [title!=a] */ div", body));
    assert!(matches(r#"div:contains("[a!=b]")"#, body));
    assert!(matches(r#":is(div:contains('x!=y'), p)"#, body));
    // `\!` is part of the identifier: `[x\!=y]` is equality on attribute `x!`.
    assert!(matches(r"[x\!=y]", "<div id=t x!=y></div>"));
    assert!(!matches(r"[x\!=y]", "<div id=t x=z></div>"));
    // Bare `!` inside a :contains argument is still rejected, never dropped.
    rejected("div:contains(!)");
    rejected("div:contains(a!=b)");
    rejected("div:contains([a!=b])");
}

#[test]
fn nested_selector_lists() {
    let body = "<section><div id=t a=v><b c=d></b></div></section>";
    for (selector, expected) in [
        (":not([a!=v])", true),
        (":not([a!=w])", false),
        (":is([a!=w], p)", true),
        (":where(p, [a!=v])", false),
        (":IS([a!=w])", true),
        (":has(> b[c!=x])", true),
        (":has(> b[c!=d])", false),
        (":has(> b:not([c!=d]))", true),
        (":has(b:is([c!=x]))", true),
        ("section > [a!=w]:not(:has([c!=d]))", true),
    ] {
        assert_eq!(matches(selector, body), expected, "{selector}");
    }
    // selectors rejects nested :has with or without the rewrite.
    let nested = Css::parse(":has(:has([c!=x]))").unwrap_err();
    assert!(Css::parse(":has(:has([c=x]))").is_err());
    assert!(!nested.contains("[attribute!=value]"), "{nested}");
}

#[test]
fn unsupported_forms_fail_explicitly() {
    for (selector, reason) in [
        ("[a!=v i]", "`i` flag"),
        ("[a!=v I]", "`i` flag"),
        ("[ns|a!=v]", "namespace"),
        ("[|a!=v]", "namespace"),
        ("[*|a!=v]", "namespace"),
        ("[*!=v]", "namespace or wildcard"),
        (r#"["a"!=v]"#, "identifier"),
        ("[a! =v]", "immediately followed"),
        ("[a!/**/=v]", "immediately followed"),
        ("[a!=]", "value must be"),
        ("[a!=v w]", "malformed"),
        ("[a!=v s s]", "malformed"),
        ("[a!=v s i]", "malformed"),
        ("[a!=1.5]", "value must be"),
        ("[a!=+1]", "value must be"),
        ("[a!=1px]", "value must be"),
        ("[a!=1e3]", "value must be"),
        ("[a!=(v)]", "value must be"),
        (r#"[a!="v]"#, "unterminated"),
        ("[a!=v", "unterminated"),
        (r"[a!=v\]", "unterminated"),
        ("[a!=--v]", "tokenizer subset"),
        ("[--a!=v]", "tokenizer subset"),
        ("[a!=\"x\\\ny\"]", "tokenizer subset"),
        ("[a!=\"x\ry\"]", "value must be"),
        ("[a!=x\0]", "tokenizer subset"),
        ("[a!=\"x\ny\"]", "value must be"),
        ("[a!!=v]", "malformed"),
        ("[!]", "malformed"),
        ("[!=v]", "malformed"),
        ("[a!]", "immediately followed"),
        ("[a!=v!=w]", "malformed"),
        (r"[xlink\:href!=v]", "qualified"),
        ("[xmlns!=v]", "qualified"),
        ("[XMLNS!=v]", "qualified"),
        ("a!", "outside an attribute"),
        ("a !b", "outside an attribute"),
        (":nth-child(1 of [a!=v])", "not in a selector list"),
        ("a:foo([a!=v])", "not in a selector list"),
        ("a([a!=v])", "not in a selector list"),
        ("[a=[b!=c]]", "outside an attribute"),
        ("[a=(b!=c)]", "outside an attribute"),
        // Forgiving lists would otherwise drop the branch silently.
        (":is([a!=v i], div)", "`i` flag"),
        (":where(div, [ns|a!=v])", "namespace"),
        (":is(div, a!)", "outside an attribute"),
    ] {
        let err = rejected(selector);
        assert!(err.contains(reason), "{selector}: {err}");
    }
}

#[test]
fn html_case_insensitive_attribute_exception() {
    // AngleSharp folds these names with .NET OrdinalIgnoreCase unless `s` is
    // given; that comparison is not ASCII folding, so it is rejected.
    for name in [
        "type",
        "lang",
        "rel",
        "target",
        "accept-charset",
        "http-equiv",
    ] {
        let err = rejected(&format!("[{name}!=x]"));
        assert!(err.contains("case-insensitive attribute"), "{err}");
        Css::parse(&format!("[{name}!=x s]")).unwrap();
    }
    // With `s`, matching is ordinal even where scraper folds by default.
    let upper = "<input id=t type=FOO>";
    assert!(matches("[type!=foo s]", upper));
    assert!(!scraper::Selector::parse(":not([type=foo])")
        .unwrap()
        .matches(&select_first(&Html::parse_document(upper), "#t")));
    assert!(!matches("[type!=FOO s]", upper));
    // The upstream name lookup is an ordinal set: `TYPE` is not listed, so it
    // compares ordinally, while GetAttribute lowercases it on HTML elements.
    assert!(matches("[TYPE!=foo]", upper));
    assert!(!matches("[TYPE!=FOO]", upper));
    assert!(matches("[Lang!=en]", "<p id=t lang=EN></p>"));
    // Names outside the list are ordinal.
    assert!(matches(
        "[cellspacing!=a]",
        "<table id=t cellspacing=A></table>"
    ));
}

#[test]
fn foreign_element_attribute_names() {
    // HTML elements: GetAttribute lowercases the name, as selectors does.
    assert!(!matches("[DATA-X!='1']", "<div id=t data-x=1></div>"));
    // Foreign elements: names are case-sensitive in both.
    let svg = "<svg><rect id=t viewBox='0 0 1 1'/></svg>";
    assert!(!matches("[viewBox!='0 0 1 1']", svg));
    assert!(matches("[viewbox!='0 0 1 1']", svg));
    assert!(matches("[VIEWBOX!='0 0 1 1']", svg));
    // Values stay ordinal on foreign elements.
    assert!(matches("rect[viewBox!='0 0 1 1 ']", svg));
    // `xlink:href` is a namespaced attribute; a bare `href` lookup misses it
    // in both (qualified name `xlink:href` versus no-namespace `href`).
    let link = "<svg><a id=t xlink:href=u href=w></a></svg>";
    assert!(matches("[href!=u]", link));
    assert!(!matches("[href!=w]", link));
    assert!(matches("[href!=u]", "<svg><a id=t xlink:href=u></a></svg>"));
}

#[test]
fn bounds_apply_to_rewritten_source() {
    // Under 4096 bytes before rewriting, over it after.
    let long = vec!["[a!=v]"; 600].concat();
    assert!(long.len() < 4096);
    let err = Css::parse(&long).unwrap_err();
    assert_eq!(
        err,
        "CSS selector length limit exceeded after [attribute!=value] rewriting"
    );
    Css::parse(&vec!["[a!=v]"; 200].concat()).unwrap();
    // Each rewrite adds one `(` level.
    let deep = |levels: usize| format!("{}[a!=v]{}", ":is(".repeat(levels), ")".repeat(levels));
    Css::parse(&deep(31)).unwrap();
    assert!(rejected(&deep(32)).contains("nesting limit exceeded"));
    // The `(` count is lexical, so string contents count; the rewrite's own
    // `:not(` then crosses the bound.
    let parens = |op: &str| format!("[b=\"{}\"][a{op}v]", "(".repeat(32));
    Css::parse(&parens("=")).unwrap();
    let err = Css::parse(&parens("!=")).unwrap_err();
    assert_eq!(
        err,
        "CSS selector nesting limit exceeded after [attribute!=value] rewriting"
    );
    // Deep blocks with a `!` fail without recursing unboundedly.
    for open in ["[", "(", "{", ":is("] {
        let err = rejected(&format!("{}!", open.repeat(5000)));
        assert!(err.contains("nesting limit exceeded"), "{err}");
    }
    // Field selectors go through the same compiled path.
    let compiled = |selector: &str| {
        Selector::compile(&parse(&format!("selector: '{selector}'"), Limits::default()).unwrap())
            .unwrap()
            .validate_html()
    };
    assert!(compiled(&deep(32)).is_err());
    assert!(compiled("td[a!=v]").is_ok());
    assert!(compiled("td[a!=v i]").is_err());
}

#[test]
fn rows_fields_cases_and_removal() {
    let body = "<table><tr id=r1 class=x><td a=keep>one<i class=drop>d</i></td></tr>\
                <tr id=r2><td a=skip>two</td></tr><tr id=r3><td>three</td></tr></table>";
    let html = Html::parse_document(body);
    let rows = selector::html_rows(&html, "tr:has(td[a!=skip])", 10).unwrap();
    let ids = rows
        .iter()
        .map(|r| r.attr("id").unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids, ["r1", "r3"]);
    let field = |yaml: &str, row: usize| {
        let row = selector::html_rows(&html, "tr", 10).unwrap()[row];
        Selector::compile(&parse(yaml, Limits::default()).unwrap())
            .unwrap()
            .extract_html(row, &|s| Ok(s.into()))
            .unwrap()
    };
    assert_eq!(
        field("selector: 'td[a!=skip]'\nremove: '[class!=x]'", 0).as_deref(),
        Some("one")
    );
    assert_eq!(field("selector: 'td[a!=skip]'", 1), None);
    let case = "case:\n  'td[a!=keep]': other\n  'td[a!=skip]': kept\n  '*': none";
    assert_eq!(field(case, 0).as_deref(), Some("kept"));
    assert_eq!(field(case, 1).as_deref(), Some("other"));
}

#[test]
fn contains_markers_and_budgets_still_apply_after_rewriting() {
    let shadow =
        "<div id=t a=v>x</div><div><template shadowrootmode=open><p>y</p></template></div>";
    let err = try_matches("[a!=w]:contains(x)", shadow).unwrap_err();
    assert!(css::is_execution_failure(&err), "{err}");
    assert!(matches("[a!=w]", shadow));
    let css = Css::parse("[a!=w]:contains(zz)").unwrap();
    let html = css::parse_document("<div id=t a=v>xxxxxxxx</div>");
    let err = css
        .matches(&select_first(&html, "#t"), &Scan::with_limit(3))
        .unwrap_err();
    assert!(css::is_execution_failure(&err), "{err}");
    // An invalid :contains is still reported as such, ahead of the selector.
    let err = Css::parse(":is([a!=v]:contains(1), p)").unwrap_err();
    assert!(err.contains("invalid :contains argument"), "{err}");
}

#[test]
fn unsupported_inequality_fails_plan_compilation() {
    let pinned = source("carpathians");
    let row = r#"selector: table[id!="torrent_ajanlo"] > tbody > tr[id]"#;
    assert!(pinned.contains(row));
    for bad in [
        r#"selector: table[id!="torrent_ajanlo" i] > tbody > tr[id]"#,
        r#"selector: table:is([id!="torrent_ajanlo" i], x) > tbody > tr[id]"#,
    ] {
        let node = parse(&pinned.replace(row, bad), Limits::default()).unwrap();
        let Err(err) = Plan::compile(&node) else {
            panic!("{bad}")
        };
        assert_eq!(err.path, "$.search.rows.selector");
        assert!(err.message.contains("`i` flag unsupported"), "{err:?}");
    }
    // Optional fields are validated at compile time too.
    let optional = pinned.replace(
        "      selector: font.line\n      filters:\n        - name: validate",
        "      selector: font[class!=line i]\n      optional: true\n      filters:\n        - name: validate",
    );
    assert_ne!(optional, pinned);
    let Err(err) = Plan::compile(&parse(&optional, Limits::default()).unwrap()) else {
        panic!("optional field compiled")
    };
    assert!(err.message.contains("[attribute!=value]"), "{err:?}");
}
