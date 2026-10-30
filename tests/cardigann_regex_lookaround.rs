//! Look-around/backreference definition regexes (docs/dotnet-regex-lookaround.md) against
//! unchanged pinned definitions. Offline compilation and synthetic responses only.
use chrono::{TimeZone, Utc};
use rackett::{
    cardigann::{self, Limits},
    cardigann_exec::Plan,
    models::SearchQuery,
};
use serde_json::json;
use std::{collections::BTreeMap, path::Path};

/// The pinned definitions whose first compile error was a regex Rust's linear engine rejects.
const REGEX_BLOCKED: [&str; 37] = [
    "abnormal-api",
    "amigosshare",
    "animetosho-xyz",
    "arabp2p",
    "arabscene",
    "astratorrent",
    "btarg",
    "c411",
    "cinemamovies",
    "crazyspirits-api",
    "devil-torrents",
    "draupnirr",
    "ebookbay",
    "electro-torrent",
    "ex-torrenty",
    "g3minitr4ck3r-api",
    "generationfree-api",
    "hawke-uno",
    "hdolimpo-api",
    "hdonly-api",
    "helltorrents",
    "lesaloon",
    "memphis",
    "nostradamus",
    "nyaasi",
    "polishtorrent",
    "polishtracker-api",
    "teamctgame",
    "teamflix",
    "theoldschool-api",
    "torrent9",
    "tr4ker",
    "v3x",
    "wihd",
    "world-torrent",
    "xtorrenty",
    "yggreborn-api",
];

fn compile(id: &str) -> Result<Plan, String> {
    let file = format!("vendor/jackett/definitions/{id}.yml");
    let node = cardigann::load(Path::new(&file), Limits::default()).map_err(|e| e.message)?;
    Plan::compile(&node).map_err(|e| format!("{}: {}", e.path, e.message))
}

#[test]
fn regex_blocked_definitions_now_pass_regex_compilation() {
    let mut still_regex = BTreeMap::new();
    let mut other = BTreeMap::new();
    for id in REGEX_BLOCKED {
        match compile(id) {
            Ok(_) => println!("{id}: compiles"),
            Err(e) if e.contains("regex") => {
                println!("{id}: still rejected: {e}");
                still_regex.insert(id, e);
            }
            Err(e) => {
                println!("{id}: regexes compile; next blocker: {e}");
                other.insert(id, e);
            }
        }
    }
    println!(
        "{}/37 definitions no longer fail on a regex",
        37 - still_regex.len()
    );
    // hawke-uno matches balanced parentheses with .NET balancing groups and a conditional.
    assert_eq!(
        still_regex,
        BTreeMap::from([(
            "hawke-uno",
            "$.search.fields.title.filters[0]: unsupported/invalid .NET regex: \
             balancing group (?<-name>)"
                .to_string()
        )])
    );
    // Only response-format blockers (another workstream) follow; none is regex-related.
    assert!(
        other.values().all(|e| e.contains("unsupported xml")),
        "{other:?}"
    );
}

/// torrent9's MULTI filter end to end: row extraction, templates and filters chained.
#[test]
fn torrent9_multi_language_titles_follow_dotnet() {
    let plan = compile("torrent9").unwrap();
    let row = |title: &str| {
        format!(
            r#"<tr><td><i class="fa fa-video-camera"></i><a href="/torrent/1">{title}</a></td><td>1.4 Go</td><td>10</td><td>2</td></tr>"#
        )
    };
    let titles = [
        "Movie.Name.MULTI.1080p.WEB-DL",
        "Movie.Name.MULTI.VOSTFR.720p",
        "Film MULTi 2160p FRENCH",
        "Film.multi.HDR",
        "Show MULTiSUBS 1080p",
    ];
    let html = format!(
        r#"<table class="table-striped"><tbody>{}</tbody></table>"#,
        titles.map(row).concat()
    );
    for (multilang, want) in [
        (
            true,
            [
                "Movie.Name.MULTi FRENCH.1080p.WEB-DL",
                "Movie.Name.MULTI.VOSTFR.720p",
                "Film MULTi 2160p FRENCH",
                "Film.MULTi FRENCH.HDR",
                "Show MULTiSUBS 1080p",
            ],
        ),
        (false, titles),
    ] {
        let ctx = plan.context(
            &BTreeMap::from([
                ("multilang".into(), json!(multilang)),
                ("multilanguage".into(), json!("MULTi FRENCH")),
            ]),
            &SearchQuery {
                params: BTreeMap::new(),
            },
            Utc.with_ymd_and_hms(2026, 1, 2, 12, 0, 0).unwrap(),
        );
        let request = plan.build_requests(&ctx).unwrap().remove(0);
        let rows = plan.extract(&request, &html, &ctx).unwrap();
        let got = rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>();
        assert_eq!(got, want, "multilang={multilang}");
    }
}
