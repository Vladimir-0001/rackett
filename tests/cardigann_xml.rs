//! Offline XML fixtures and compilation delta for the pinned fe8876d2 corpus.
use chrono::{TimeZone, Utc};
use rackett::{
    cardigann::{load, parse, Limits},
    cardigann_exec::Plan,
    models::{Release, SearchQuery},
};
use std::{collections::BTreeMap, path::Path};

const IDS: [&str; 18] = [
    "anibt",
    "binarybay",
    "bitagent",
    "bitmagnet",
    "bluetorrents",
    "dasunerwartete-api",
    "fileiplay",
    "nekobt",
    "pandacd",
    "periodical",
    "pornotorrent",
    "retrotoon",
    "scenetime-api",
    "shadowbit",
    "showrss",
    "stellarwinds",
    "torrentclaw",
    "world-of-tomorrow-api",
];

fn extract(
    plan: &Plan,
    body: &str,
) -> Result<Vec<Release>, rackett::cardigann_exec::ExecutionError> {
    let ctx = plan.context(
        &BTreeMap::new(),
        &SearchQuery::default(),
        Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap(),
    );
    let request = plan.build_requests(&ctx).unwrap().remove(0);
    assert_eq!(request.response_type, "xml");
    plan.extract(&request, body, &ctx)
}

fn pinned(id: &str) -> Plan {
    Plan::compile(
        &load(
            Path::new(&format!("vendor/jackett/definitions/{id}.yml")),
            Limits::default(),
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn pinned_xml_compilation_delta() {
    let mut compiled = Vec::new();
    let mut blocked = Vec::new();
    for id in IDS {
        let node = load(
            Path::new(&format!("vendor/jackett/definitions/{id}.yml")),
            Limits::default(),
        )
        .unwrap();
        match Plan::compile(&node) {
            Ok(_) => compiled.push(id),
            Err(e) => blocked.push(format!("{id}: {e}")),
        }
    }
    eprintln!(
        "XML plans compiled ({}/18): {}",
        compiled.len(),
        compiled.join(", ")
    );
    eprintln!("Remaining blockers: {blocked:#?}");
    assert_eq!(compiled.len(), 17, "{blocked:#?}");
    assert_eq!(blocked, ["dasunerwartete-api: $.search.fields._onlyup: unsupported CSS selector \"free_button:contains(Only Upload)\": invalid :contains argument: argument must be exactly one token"]);
}

#[test]
fn torznab_feed_through_pinned_bitmagnet_plan() {
    let rows = extract(
        &pinned("bitmagnet"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss xmlns:torznab="http://torznab.com/schemas/2015/feed" version="2.0"><channel>
  <title>Bitmagnet</title><item>
    <title><![CDATA[Fixture <1080p> & Release]]></title>
    <category>Movies</category><pubDate>Thu, 29 Oct 2026 10:00:00 +0000</pubDate>
    <guid>0123456789abcdef0123456789abcdef01234567</guid><size>1048576</size>
    <torznab:attr name="seeders" value="12"/><torznab:attr name="leechers" value="3"/>
  </item></channel></rss>"#,
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "Fixture <1080p> & Release");
    assert_eq!(rows[0].seeders, Some(12));
    assert_eq!(rows[0].peers, Some(15));
    assert_eq!(rows[0].size, Some(1048576));
    assert_eq!(
        rows[0].attributes["infohash"],
        "0123456789abcdef0123456789abcdef01234567"
    );
    assert_eq!(rows[0].published.as_deref(), Some("2026-10-29T10:00:00Z"));
}

fn synthetic(encoding: &str, extra: &str, fields: &str) -> Plan {
    let source = format!(
        r#"id: rss
name: RSS
description: Fixture
language: en-US
type: public
encoding: {encoding}
links: [https://example.org/]
caps:
  categories: {{Other: Other}}
  modes: {{search: [q]}}
search:
  paths: [{{path: rss, response: {{type: xml}}}}]
{extra}
  rows: {{selector: 'rss > channel > item'}}
  fields:
    category: {{text: Other}}
    size: {{selector: enclosure, attribute: length}}
    seeders: {{text: 1}}
    leechers: {{text: 0}}
    date: {{selector: pubDate}}
    download: {{selector: enclosure, attribute: url}}
    title: {{selector: title}}
{fields}
"#
    );
    Plan::compile(&parse(&source, Limits::default()).unwrap()).unwrap()
}

#[test]
fn rss_enclosure_entities_link_and_cdata_remain_xml_text() {
    let plan = synthetic("UTF-8", "", "    description: {selector: description}");
    let rows = extract(&plan, r#"<?xml version="1.0"?><rss><channel><item>
      <title>Fixture &#x26; Release</title><link>https://example.org/details/42</link>
      <description><![CDATA[<b>literal</b> &amp; data]]><!--ignored--> tail &lt;end&gt;</description>
      <enclosure url="/download/42?a=1&amp;b=2" length="2048" type="application/x-bittorrent"/>
      <pubDate>Thu, 29 Oct 2026 10:00:00 +0000</pubDate>
    </item></channel></rss>"#).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "Fixture & Release");
    assert_eq!(rows[0].size, Some(2048));
    assert_eq!(rows[0].link, "https://example.org/download/42?a=1&b=2");
    assert_eq!(
        rows[0].attributes["description"],
        "<b>literal</b> &amp; data tail <end>"
    );
}

#[test]
fn definition_charset_wins_over_xml_encoding_declaration() {
    let plan = synthetic("iso-8859-1", "", "");
    let body = b"<?xml version='1.0' encoding='windows-1251'?><rss><channel><item><title>caf\xe9</title><enclosure url='/file' length='0'/><pubDate>2026-10-29T10:00:00Z</pubDate></item></channel></rss>";
    let decoded = plan.charset().decode(body.to_vec()).unwrap();
    assert_eq!(extract(&plan, &decoded).unwrap()[0].title, "café");
}

#[test]
fn preprocessing_is_applied_before_xml_selection() {
    let plan = synthetic(
        "UTF-8",
        "  preprocessingfilters: [{name: replace, args: [before, after]}]",
        "",
    );
    let rows = extract(&plan, "<rss><channel><item><title>before</title><enclosure url='/file' length='0'/><pubDate>2026-10-29T10:00:00Z</pubDate></item></channel></rss>").unwrap();
    assert_eq!(rows[0].title, "after");
}

#[test]
fn xml_fields_share_mutation_cases_root_and_structural_selectors() {
    let plan = synthetic(
        "UTF-8",
        "",
        r#"    scratch|optional: {selector: description, remove: omit}
    description: {selector: description, case: {':has(keep):contains(AB)': retained}}
    author: {selector: ':root channel > owner', attribute: Name}
    publisher: {selector: 'description > keep:first-child', attribute: Value}
"#,
    );
    let rows = extract(&plan, r#"<rss><channel><owner Name="Editor"/><item>
      <title>Fixture</title><description><omit>discard</omit><keep Value="exact">A</keep>B</description>
      <enclosure url="/file" length="1"/><pubDate>2026-10-29T10:00:00Z</pubDate>
    </item></channel></rss>"#).unwrap();
    assert_eq!(rows[0].attributes["description"], "retained");
    assert_eq!(rows[0].attributes["author"], "Editor");
    assert_eq!(rows[0].attributes["publisher"], "exact");
}

#[test]
fn malformed_raw_xml_cannot_be_repaired_by_preprocessing() {
    let plan = synthetic(
        "UTF-8",
        "  preprocessingfilters: [{name: replace, args: [broken, '<rss/>']}]",
        "",
    );
    assert_eq!(extract(&plan, "broken").unwrap_err().path, "$.response");
}

#[test]
fn xml_retains_field_text_and_row_limits() {
    let plan = synthetic("UTF-8", "", "");
    let text = "x".repeat(1_048_577);
    for title in [text.clone(), format!("<![CDATA[{text}]]>")] {
        let body = format!("<rss><channel><item><title>{title}</title><enclosure url='/file' length='0'/><pubDate>2026-10-29T10:00:00Z</pubDate></item></channel></rss>");
        let err = extract(&plan, &body).unwrap_err();
        assert_eq!(err.path, "$.search.fields.title");
        assert_eq!(err.message, "selector output limit exceeded");
    }
    let body = format!("<rss><channel>{}</channel></rss>", "<item/>".repeat(1001));
    let err = extract(&plan, &body).unwrap_err();
    assert_eq!(err.path, "$.search.rows");
    assert!(err.message.contains("row limit"));
}

#[test]
fn malformed_xml_is_a_response_error_even_with_optional_fields() {
    let plan = synthetic("UTF-8", "", "    scratch|optional: {selector: x}");
    for body in [
        "<rss>",
        "<rss><channel></rss>",
        "<rss>&nbsp;</rss>",
        "<!DOCTYPE rss [<!ENTITY x 'expanded'>]><rss>&x;</rss>",
    ] {
        let err = extract(&plan, body).unwrap_err();
        assert_eq!(err.path, "$.response");
        assert!(err.message.starts_with("Invalid XML"));
    }
}
