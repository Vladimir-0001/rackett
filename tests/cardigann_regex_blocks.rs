//! Synthetic responses against unchanged pinned definitions. No live availability claim.
use chrono::{TimeZone, Utc};
use rackett::{
    cardigann::{self, Limits},
    cardigann_exec::Plan,
    models::SearchQuery,
};
use serde_json::json;
use std::{collections::BTreeMap, path::Path};

fn check(id: &str, html: &str) {
    let file = format!("vendor/jackett/definitions/{id}.yml");
    let p = Plan::compile(&cardigann::load(Path::new(&file), Limits::default()).unwrap()).unwrap();
    if id == "animebybelka" {
        assert!(!p.blockers().is_empty(), "login must remain blocked");
    }
    for stripped in [false, true] {
        let ctx = p.context(
            &BTreeMap::from([
                ("stripcyrillic".into(), json!(stripped)),
                ("addrussiantotitle".into(), json!(false)),
                ("useragent".into(), json!("Offline fixture")),
            ]),
            &SearchQuery {
                params: BTreeMap::from([("q".into(), "Title".into())]),
            },
            Utc.with_ymd_and_hms(2026, 1, 2, 12, 0, 0).unwrap(),
        );
        let req = p.build_requests(&ctx).unwrap().remove(0);
        let rows = p.extract(&req, html, &ctx).unwrap();
        assert_eq!(rows.len(), 1, "{id}");
        assert_eq!(
            rows[0].title,
            if stripped {
                "Title Ԁ End"
            } else {
                "Title АԀ End"
            },
            "{id} strip={stripped}"
        );
    }
}

#[test]
fn animebybelka_strips_only_the_dotnet_cyrillic_block() {
    check(
        "animebybelka",
        r#"<table><tbody><tr>
      <td>Description <a href="./viewforum.php?f=32">Anime</a>
      <a class="topictitle" href="./viewtopic.php?t=1">Title АԀ End</a>
      <a href="./download/file.php?id=1">Download</a></td>
      <td><p><b>1 GB</b></p></td><td><span class="seed">3</span></td>
      <td><span class="leech">2</span><span class="complet">1</span></td><td></td>
    </tr></tbody></table>"#,
    );
}

#[test]
fn rutracker_strips_only_the_dotnet_cyrillic_block() {
    check(
        "rutracker-ru",
        r#"<table><tbody><tr id="tor_1">
      <td><a href="tracker.php?f=1737">Movies</a>
      <a href="./viewtopic.php?t=1">Title АԀ End</a>
      <a href="magnet:?xt=urn:btih:0123456789012345678901234567890123456789">Magnet</a></td>
      <td></td><td></td><td></td><td></td><td><u>1024</u></td>
      <td class="seedmed"><b>3</b></td><td class="leechmed"><b>2</b></td>
      <td>1</td><td><u>1700000000</u></td>
    </tr></tbody></table>"#,
    );
}
