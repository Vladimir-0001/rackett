//! Synthetic fixtures for HTML `search.rows.after`. Source: Jackett fe8876d2
//! `CardigannIndexer.cs:1819-1852` (snapshot `ToList`, per-head `Append` of the next
//! rows' `ChildNodes`, `RemoveRange` throwing on an incomplete final group, all before
//! field extraction) and `IndexerDefinition.cs:154-160` (`int After`). AngleSharp
//! 9253950e `ParentNodeExtensions.cs:19-34,55-62` and `NodeExtensions.cs:408-413`
//! (moves, `HierarchyRequest` for an ancestor). No live responses or credentials.
use chrono::{TimeZone, Utc};
use rackett::{
    cardigann::{parse, Limits},
    cardigann_exec::{
        selector::{html_row_slots, merge_after_rows},
        ExecutionError, Plan,
    },
    models::{Release, SearchQuery},
};
use scraper::Html;
use std::collections::BTreeMap;

const BUDGET: usize = 20_000_000;

fn definition(paths: &str, rows: &str, fields: &str) -> String {
    format!(
        r#"id: after
name: After
description: Synthetic
language: en-US
type: public
encoding: UTF-8
links: [https://example.org/]
caps:
  categories: {{Other: Other}}
  modes: {{search: [q]}}
search:
  paths: {paths}
  rows:
{rows}
  fields:
    category: {{text: Other}}
    size: {{text: 0}}
    seeders: {{text: 0}}
    leechers: {{text: 0}}
    date: {{text: 2024-02-29}}
    download: {{text: /download}}
{fields}
"#
    )
}
fn html_plan(rows: &str, fields: &str) -> Result<Plan, ExecutionError> {
    compile(&definition("[{path: search}]", rows, fields))
}
fn compile(source: &str) -> Result<Plan, ExecutionError> {
    Plan::compile(&parse(source, Limits::default()).unwrap())
}
fn vendor(name: &str) -> Plan {
    compile(&std::fs::read_to_string(format!("vendor/jackett/definitions/{name}.yml")).unwrap())
        .unwrap()
}
fn run_query(plan: &Plan, q: &str, body: &str) -> Result<Vec<Release>, ExecutionError> {
    let query = SearchQuery {
        params: BTreeMap::from([("q".into(), q.into()), ("t".into(), "search".into())]),
    };
    let context = plan.context(
        &BTreeMap::new(),
        &query,
        Utc.with_ymd_and_hms(2026, 10, 28, 12, 0, 0).unwrap(),
    );
    let requests = plan.build_requests(&context).unwrap();
    plan.extract(&requests[0], body, &context)
}
fn run(plan: &Plan, body: &str) -> Result<Vec<Release>, ExecutionError> {
    run_query(plan, "", body)
}
fn page(body: &str) -> String {
    format!("<!DOCTYPE html><html><head></head><body>{body}</body></html>")
}
fn attr<'a>(release: &'a Release, key: &str) -> Option<&'a str> {
    release.attributes.get(key).map(String::as_str)
}
fn failure(result: Result<Vec<Release>, ExecutionError>, path: &str, needle: &str) {
    let e = result.unwrap_err();
    assert_eq!(e.path, path, "{e}");
    assert!(e.message.contains(needle), "{e}");
}
/// Merge directly, returning the document so parent links can be inspected.
fn merge(body: &str, rows: &str, after: usize) -> (Html, Result<Vec<usize>, String>, usize) {
    let mut html = Html::parse_document(body);
    let slots = html_row_slots(&html, rows, 1000).unwrap();
    let mut visits = BUDGET;
    let merged = merge_after_rows(&mut html, slots, after, &mut visits);
    (html, merged, BUDGET - visits)
}
/// Arena node at a row slot (ego-tree is not a direct dependency to name its type).
macro_rules! node {
    ($html:expr, $slot:expr) => {
        $html.tree.nodes().nth($slot).unwrap()
    };
}
fn texts(html: &Html, slot: usize) -> String {
    scraper::ElementRef::wrap(node!(html, slot))
        .unwrap()
        .text()
        .collect()
}

const TWO_PAIRS: &str = "<table>\
    <tr class=h><td>A1</td><td>B1</td></tr>\
    <tr class=s><td>C1</td><td class=mid>M1</td><td>Z1</td></tr>\
    <tr class=h><td>A2</td><td>B2</td></tr>\
    <tr class=s><td>C2</td><td class=mid>M2</td><td>Z2</td></tr></table>";

#[test]
fn four_rows_after_one_yield_two_merged_releases_with_live_parents() {
    let plan = html_plan(
        "    selector: tr\n    after: 1",
        "    title: {selector: 'td:nth-child(3)'}
    description: {selector: 'tr.h > td.mid'}
    author: {selector: 'td:last-child'}
    publisher: {selector: 'td:nth-child(4)'}",
    )
    .unwrap();
    let releases = run(&plan, &page(TWO_PAIRS)).unwrap();
    assert_eq!(releases.len(), 2);
    for (i, r) in releases.iter().enumerate() {
        let n = i + 1;
        // Two head cells plus the first moved cell: nth-child(3) is the moved C.
        assert_eq!(r.title, format!("C{n}"));
        // `tr.h >` requires the moved middle cell's parent link to be the head.
        assert_eq!(attr(r, "description"), Some(format!("M{n}").as_str()));
        assert_eq!(attr(r, "author"), Some(format!("Z{n}").as_str()));
        assert_eq!(attr(r, "publisher"), Some(format!("M{n}").as_str()));
    }
    // Without merging the moved cells are absent from each head row.
    let plain = html_plan(
        "    selector: tr.h",
        "    title: {selector: 'td:nth-child(3)'}",
    )
    .unwrap();
    failure(
        run(&plain, &page(TWO_PAIRS)),
        "$.search.fields.title",
        "did not match",
    );

    let (html, merged, _) = merge(&page(TWO_PAIRS), "tr", 1);
    let heads = merged.unwrap();
    assert_eq!(heads.len(), 2);
    for &head in &heads {
        let head_node = node!(html, head);
        let children = head_node.children().collect::<Vec<_>>();
        assert_eq!(children.len(), 5);
        for child in children {
            assert_eq!(child.parent().unwrap().id(), head_node.id());
        }
        // Sibling links run through the moved cells in order and back again.
        let last = head_node.last_child().unwrap();
        assert_eq!(last.prev_siblings().count(), 4);
        assert!(last.next_sibling().is_none());
    }
}

#[test]
fn after_two_appends_children_in_row_order_and_empties_merged_rows() {
    let body = page(
        "<div class=r><b>r0</b></div><div class=r><i>r1</i></div><div class=r><u>r2</u></div>",
    );
    let plan = html_plan(
        "    selector: div.r\n    after: 2",
        r#"    title: {}
    author: {selector: 'div.r > :last-child'}
    description:
      case:
        ":root div.r:not(:empty) ~ div.r:not(:empty)": leaked
        ":root div.r:first-child + div.r:empty + div.r:empty": emptied
        "*": missing"#,
    )
    .unwrap();
    let releases = run(&plan, &body).unwrap();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].title, "r0r1r2");
    assert_eq!(attr(&releases[0], "author"), Some("r2"));
    // Merged-away rows stay in the DOM, empty, as the following siblings.
    assert_eq!(attr(&releases[0], "description"), Some("emptied"));

    let (html, merged, _) = merge(&body, "div.r", 2);
    let slots = html_row_slots(&Html::parse_document(&body), "div.r", 10).unwrap();
    assert_eq!(merged.unwrap(), [slots[0]]);
    assert_eq!(texts(&html, slots[0]), "r0r1r2");
    for &slot in &slots[1..] {
        let row = node!(html, slot);
        assert!(row.first_child().is_none());
        assert!(row.parent().is_some(), "merged row stays attached");
    }
}

#[test]
fn incomplete_groups_fail_the_whole_response_and_zero_rows_are_empty() {
    let rows = "<p class=r>a</p><p class=r>b</p><p class=r>c</p>";
    let plan = html_plan("    selector: p.r\n    after: 1", "    title: {}").unwrap();
    for body in ["<p class=r>a</p>", rows] {
        // N = 1 or 3: no partial release for the complete leading group.
        failure(run(&plan, &page(body)), "$.search.rows.after", "Incomplete");
    }
    assert!(run(&plan, &page("<p>none</p>")).unwrap().is_empty());
    let max = html_plan("    selector: p.r\n    after: 2147483647", "    title: {}").unwrap();
    failure(run(&max, &page(rows)), "$.search.rows.after", "Incomplete");
    assert!(run(&max, &page("<p>none</p>")).unwrap().is_empty());
    let (_, merged, used) = merge(&page(rows), "p.r", usize::MAX);
    assert!(merged.unwrap_err().contains("Incomplete"));
    assert_eq!(used, 0, "group size checked before any work");
    let (_, merged, used) = merge(&page(""), "p.r", usize::MAX);
    assert!(merged.unwrap().is_empty());
    assert_eq!(used, 0);
}

#[test]
fn non_positive_after_matches_absence_and_malformed_values_reject() {
    let fields = "    title: {}\n    description: {selector: b}";
    let body = page("<p class=r>a<b>1</b></p><p class=r>b<b>2</b></p><p class=r>c</p>");
    let absent = format!(
        "{:?}",
        run(&html_plan("    selector: p.r", fields).unwrap(), &body).unwrap()
    );
    assert!(absent.contains("\"a1\"") && absent.contains("\"c\""));
    for value in ["0", "-1", "-0", "-2147483648"] {
        let plan = html_plan(&format!("    selector: p.r\n    after: {value}"), fields).unwrap();
        assert_eq!(
            format!("{:?}", run(&plan, &body).unwrap()),
            absent,
            "{value}"
        );
    }
    for value in [
        "x",
        "1.5",
        "[1]",
        "{n: 1}",
        "2147483648",
        "-2147483649",
        "'1'",
        "\"1\"",
        "+1",
        "01",
        "0x1",
        "1_0",
        "",
        "~",
    ] {
        let e = html_plan(&format!("    selector: p.r\n    after: {value}"), fields)
            .err()
            .unwrap_or_else(|| panic!("{value:?} compiled"));
        assert_eq!(e.path, "$.search.rows.after", "{value:?}: {e}");
    }
    let e = html_plan("    selector: p.r\n    after: 1\n    after: 1", fields)
        .err()
        .unwrap();
    assert_eq!(e.path, "$.search.rows.after");
    assert!(e.message.contains("Ambiguous"), "{e}");

    let json_fields = "    title: {selector: name}";
    let json = "[{path: search, response: {type: json}}]";
    let mixed = "[{path: search}, {path: api, response: {type: json}}]";
    for paths in [json, mixed] {
        let e = compile(&definition(
            paths,
            "    selector: data\n    after: 1",
            json_fields,
        ))
        .err()
        .unwrap();
        assert_eq!(e.path, "$.search.rows.after", "{e}");
    }
    for value in ["0", "-3"] {
        let plan = compile(&definition(
            json,
            &format!("    selector: data\n    after: {value}"),
            json_fields,
        ))
        .unwrap();
        let releases = run(&plan, r#"{"data":[{"name":"a"},{"name":"b"}]}"#).unwrap();
        assert_eq!(releases.len(), 2);
    }
}

#[test]
fn nested_source_children_become_direct_head_children_and_cycles_fail() {
    let body = page("<div class=r><i>a</i><div class=r><i>b</i></div></div>");
    let plan = html_plan(
        "    selector: div.r\n    after: 1",
        "    title: {selector: 'div.r + i'}",
    )
    .unwrap();
    let releases = run(&plan, &body).unwrap();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].title, "b");

    let (html, merged, _) = merge(&body, "div.r", 1);
    let [outer] = merged.unwrap()[..] else {
        panic!("one head")
    };
    let outer = node!(html, outer);
    let names = outer
        .children()
        .map(|c| {
            assert_eq!(c.parent().unwrap().id(), outer.id());
            c.value().as_element().unwrap().name().to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(names, ["i", "div", "i"]);

    let mut html = Html::parse_document(&body);
    let slots = html_row_slots(&html, "div.r", 10).unwrap();
    let mut visits = BUDGET;
    let reversed = vec![slots[1], slots[0]];
    assert_eq!(
        merge_after_rows(&mut html, reversed, 1, &mut visits).unwrap_err(),
        "Row merge would create a cycle"
    );
    let same = vec![slots[0], slots[0]];
    assert_eq!(
        merge_after_rows(&mut html, same, 1, &mut visits).unwrap_err(),
        "Row merge source is its own target"
    );
}

#[test]
fn templates_depth_and_budget_fail_closed() {
    let plan = html_plan("    selector: .r\n    after: 1", "    title: {}").unwrap();
    for body in [
        "<div class=r>a</div><template class=r><b>x</b></template>",
        "<template class=r><b>x</b></template><div class=r>a</div>",
    ] {
        failure(run(&plan, &page(body)), "$.search.rows.after", "Template");
    }
    // Both rows are within the depth bound until the deep source subtree moves.
    let deep = format!(
        "{}<p class=r>h</p>{}<p class=r>{}x{}</p>",
        "<div>".repeat(110),
        "</div>".repeat(110),
        "<span>".repeat(40),
        "</span>".repeat(40)
    );
    failure(run(&plan, &page(&deep)), "$.response", "depth");
    let shallow = html_plan("    selector: .r", "    title: {}").unwrap();
    assert_eq!(run(&shallow, &page(&deep)).unwrap().len(), 2);

    // Exact charge: arena map (11 nodes), inert-ancestry preflight over both rows
    // (3 + 3), head cycle walk (3), moved children (2).
    let body = "<div class=r>a</div><div class=r><b>1</b><b>2</b></div>";
    let (_, merged, used) = merge(body, ".r", 1);
    merged.unwrap();
    assert_eq!(used, 22);
    // 21 fails in the moves, 16 inside the preflight, 10 at the arena map.
    for (budget, ok) in [(22, true), (21, false), (16, false), (10, false)] {
        let mut html = Html::parse_document(body);
        let slots = html_row_slots(&html, ".r", 10).unwrap();
        let mut visits = budget;
        let merged = merge_after_rows(&mut html, slots, 1, &mut visits);
        assert_eq!(merged.is_ok(), ok, "{budget}");
        if !ok {
            assert!(merged.unwrap_err().contains("aggregate visit budget"));
        }
    }
}

const INERT: &str = "Inert template row merge unsupported";

#[test]
fn rows_inside_template_contents_reject_positive_after_merges() {
    // AngleSharp QuerySelectorAll never selects template contents, but html_select
    // descends into scraper's Fragment; merging would pair rows upstream never sees.
    let plan = html_plan("    selector: div.r\n    after: 1", "    title: {}").unwrap();
    let repro =
        "<template><div class=r>T</div></template><div class=r>A</div><div class=r>B</div><div class=r>C</div>";
    for body in [
        // Review repro: upstream selects A/B/C and throws on the incomplete group.
        repro,
        // Only the source is inert, after an ordinary head.
        "<div class=r>A</div><template><div class=r>T</div></template>",
        // Nested templates, with intermediate ordinary elements.
        "<template><section><template><div class=r>T</div></template></section></template>\
         <div class=r>A</div>",
        // Even count overall: real rows A/B would pair, then inert T1/T2.
        "<div class=r>A</div><div class=r>B</div>\
         <template><div class=r>T1</div><div class=r>T2</div></template>",
    ] {
        let e = run(&plan, &page(body)).unwrap_err();
        assert_eq!(e.path, "$.search.rows.after", "{body}");
        // Static message: no row text, selector or response content.
        assert_eq!(e.message, INERT, "{body}");
    }
    let mut html = Html::parse_document(&page(repro));
    let slots = html_row_slots(&html, "div.r", 10).unwrap();
    let before = html.html();
    let mut visits = BUDGET;
    assert_eq!(
        merge_after_rows(&mut html, slots, 1, &mut visits).unwrap_err(),
        INERT
    );
    assert_eq!(html.html(), before, "rejected before any move");

    // Controls: absent and zero `after` keep the current selection, template rows included.
    let absent = run(
        &html_plan("    selector: div.r", "    title: {}").unwrap(),
        &page(repro),
    )
    .unwrap();
    assert_eq!(
        absent.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
        ["T", "A", "B", "C"]
    );
    let zero = html_plan("    selector: div.r\n    after: 0", "    title: {}").unwrap();
    assert_eq!(
        format!("{:?}", run(&zero, &page(repro)).unwrap()),
        format!("{absent:?}")
    );
    // Ordinary selected rows that merely contain templates still merge as before.
    for (body, title) in [
        (
            "<div class=r>A<template><b>t</b></template></div><div class=r>B</div>",
            "AtB",
        ),
        (
            "<div class=r>A</div><div class=r>B<template><b>t</b></template></div>",
            "ABt",
        ),
    ] {
        let releases = run(&plan, &page(body)).unwrap();
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].title, title);
    }
}

#[test]
fn shared_visit_budget_and_raw_row_limit_still_apply_after_merging() {
    // Heads sit after ~90k arena slots, so each selector field's row lookup is costly.
    let rows = |n: usize| {
        page(&format!(
            "{}{}",
            "<i></i>".repeat(90_000),
            "<p class=r><b>t</b></p>".repeat(n)
        ))
    };
    let plan = html_plan(
        "    selector: p.r\n    after: 1",
        "    title: {}\n    description: {selector: b}\n    author: {selector: b}",
    )
    .unwrap();
    assert_eq!(run(&plan, &rows(50)).unwrap().len(), 25);
    failure(
        run(&plan, &rows(200)),
        "$.response",
        "aggregate visit budget",
    );
    // The 1000-row bound applies to the raw selection, before merging halves it.
    let small = page(&"<p class=r>t</p>".repeat(1002));
    failure(run(&plan, &small), "$.search.rows", "row limit");
}

#[test]
fn fields_see_all_groups_merged_before_any_mutation() {
    let plan = html_plan(
        "    selector: div.r\n    after: 1",
        "    title: {remove: b}",
    )
    .unwrap();
    let body = page("<div class=r>keep</div><div class=r><b>junk</b> more</div>");
    assert_eq!(run(&plan, &body).unwrap()[0].title, "keep more");

    let plan = html_plan(
        "    selector: div.r\n    after: 1",
        r#"    title: {}
    description:
      case:
        ":root #h2 > b.m2": merged
        "*": pending"#,
    )
    .unwrap();
    let body = page(
        "<div class=r>h1</div><div class=r><b class=m1>x</b></div>\
         <div class=r id=h2>h2</div><div class=r><b class=m2>y</b></div>",
    );
    let releases = run(&plan, &body).unwrap();
    assert_eq!(releases.len(), 2);
    assert_eq!(attr(&releases[0], "description"), Some("merged"));

    // C is nested in source B and moves under A, then still heads its own group;
    // the first release's removal detaches it, and its slot still extracts.
    let plan = html_plan(
        "    selector: div.r\n    after: 1",
        "    description: {remove: div.r}\n    title: {}",
    )
    .unwrap();
    let body =
        page("<div class=r>A</div><div class=r>B<div class=r>C</div></div><div class=r>D</div>");
    let releases = run(&plan, &body).unwrap();
    assert_eq!(releases.len(), 2);
    assert_eq!(releases[0].title, "AB");
    assert_eq!(releases[1].title, "CD");
}

#[test]
fn tokyotosho_pairs_category_rows() {
    let plan = vendor("tokyotosho");
    assert!(plan.blockers().is_empty());
    let group = |id: u32, cat: u32, title: &str, size: &str| {
        format!(
            "<tr class=\"category_0\"><td rowspan=\"2\"><a href=\"/?cat={cat}\">c</a></td>\
             <td class=\"desc-top\"><a href=\"magnet:?xt=urn:btih:{id:040}\">m</a> \
             <a href=\"https://dl.example/{id}.torrent\" type=\"application/x-bittorrent\">{title}</a></td>\
             <td class=\"web\"><a href=\"details.php?id={id}\">Details</a></td></tr>\
             <tr class=\"category_0\"><td class=\"desc-bot\">Submitter: x | Size: {size} | \
             Date: 2024-05-01 12:34 UTC | Comment: y</td>\
             <td class=\"stats\">S: <span>1{id}</span> L: <span>{id}</span> C: <span>9</span></td></tr>"
        )
    };
    let body = page(&format!(
        "<table class=\"listing\">{}{}{}</table>",
        group(1, 1, "[Sub] Aria The Animation 01", "1.2GB"),
        group(2, 3, "Unrelated Manga v02", "300MB"),
        group(3, 5, "Aria The Natural 02", "512MB"),
    ));
    let releases = run_query(&plan, "aria", &body).unwrap();
    // andmatch runs after merging and field extraction.
    assert_eq!(releases.len(), 2);
    let r = &releases[0];
    assert_eq!(r.title, "[Sub] Aria The Animation 01");
    assert_eq!(r.link, "https://dl.example/1.torrent");
    assert_eq!(
        r.details.as_deref(),
        Some("https://www.tokyotosho.info/details.php?id=1")
    );
    assert_eq!(r.size, Some(1_288_490_188));
    assert_eq!(r.published.as_deref(), Some("2024-05-01T12:34:00Z"));
    assert_eq!((r.seeders, r.peers), (Some(11), Some(12)));
    assert!(r.categories.contains(&5070));
    assert_eq!(releases[1].title, "Aria The Natural 02");
    assert_eq!(releases[1].size, Some(512 * 1024 * 1024));
    assert!(releases[1].categories.contains(&8000));
    // An unpaired trailing row fails the response rather than dropping it.
    let odd = body.replace(
        "</table>",
        "<tr class=\"category_0\"><td>x</td></tr></table>",
    );
    failure(
        run_query(&plan, "aria", &odd),
        "$.search.rows.after",
        "Incomplete",
    );
}

#[test]
fn indietorrents_merges_three_rows_before_removals() {
    let plan = vendor("indietorrents");
    let body = page(
        "<table id=\"torrent_table\"><tbody>\
         <tr class=\"group\"><td class=\"center\"></td><td class=\"cats_col\"></td>\
         <td>Lily Konigsberg - <a href=\"torrents.php?id=7\">We Need To Talk Now</a> \
         <span>[2021]</span><div class=\"tags\">indie, pop</div></td></tr>\
         <tr class=\"group_edition\"><td class=\"edition_info\"><strong>2021 - Digital</strong></td></tr>\
         <tr class=\"group_torrent\"><td><a href=\"torrents.php?action=download&amp;id=55\">DL</a> \
         <a href=\"torrents.php?id=7&amp;torrentid=55\">FLAC / Lossless <strong>Freeleech!</strong></a></td>\
         <td>12</td><td><span class=\"time\" title=\"Oct 3 2024, 14:05\">1 day ago</span></td>\
         <td>300.5 MB</td><td>4</td><td>9</td><td>1</td><td>x</td></tr>\
         </tbody></table>",
    );
    let releases = run_query(&plan, "lily", &body).unwrap();
    assert_eq!(releases.len(), 1);
    let r = &releases[0];
    // title_details removes span/div.tags only after _tags read them.
    assert_eq!(
        r.title,
        "Lily Konigsberg - We Need To Talk Now FLAC / Lossless"
    );
    assert_eq!(
        attr(r, "description"),
        Some("2021 - Digital</br>indie, pop")
    );
    assert_eq!(
        r.link,
        "https://www.indietorrents.com/torrents.php?action=download&id=55"
    );
    assert_eq!(
        r.details.as_deref(),
        Some("https://www.indietorrents.com/torrents.php?id=7")
    );
    assert_eq!(attr(r, "files"), Some("12"));
    assert_eq!(attr(r, "grabs"), Some("4"));
    assert_eq!(r.size, Some(315_097_088));
    assert_eq!((r.seeders, r.peers), (Some(9), Some(10)));
    assert_eq!(r.published.as_deref(), Some("2024-10-03T14:05:00Z"));
    // The freeleech case is read before title's `remove: strong`.
    assert_eq!(attr(r, "downloadvolumefactor"), Some("0"));
    assert!(r.categories.contains(&3000));
    let short = body.replace(
        "<tr class=\"group_edition\"><td class=\"edition_info\"><strong>2021 - Digital</strong></td></tr>",
        "",
    );
    failure(
        run_query(&plan, "lily", &short),
        "$.search.rows.after",
        "Incomplete",
    );
}

#[test]
fn torrentcore_purple_head_includes_itself_after_merging() {
    let plan = vendor("torrentcore");
    let group = |cat: &str, title: &str| {
        format!(
            "<font color=\"purple\">Size: 1.4 GB/ File: 3 [ 01 October 2024 12:00:00 ]</font><br>\
             <font color=\"orange\"><img src=\"./cat/{cat}.png\"> <a href=\"/torrent/{cat}\">{title}</a></font><br>\
             <font color=\"gray\">Hash: 0123</font><font color=\"white\">peers</font>"
        )
    };
    let body = page(&format!(
        "<div class=\"content\">{}{}</div>",
        group("X264", "Ubuntu Movie"),
        group("EBOOK", "Ubuntu Book")
    ));
    let releases = run_query(&plan, "ubuntu", &body).unwrap();
    assert_eq!(releases.len(), 2);
    let r = &releases[0];
    assert_eq!(r.title, "Ubuntu Movie");
    assert_eq!(r.link, "https://torrentcore.xyz/torrent/X264");
    assert_eq!(r.size, Some(1_503_238_553));
    assert_eq!(attr(r, "files"), Some("3"));
    assert_eq!(r.published.as_deref(), Some("2024-10-01T12:00:00Z"));
    assert!(r.categories.contains(&2040));
    assert!(releases[1].categories.contains(&7000));
    // Raw `:root:contains` error blocks run before rows are merged.
    let error =
        page("<div class=\"content\"><font color=\"purple\">x</font>Service Unavailable</div>");
    let e = run_query(&plan, "ubuntu", &error).unwrap_err();
    assert_eq!(e.path, "$.search.error[0]", "{e}");
    let odd = page("<div class=\"content\"><font color=\"purple\">x</font></div>");
    failure(
        run_query(&plan, "ubuntu", &odd),
        "$.search.rows.after",
        "Incomplete",
    );
}
