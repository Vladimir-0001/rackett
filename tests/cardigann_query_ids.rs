//! Pinned CardigannIndexer.cs:1459–1466: absent ID variables equal .False (null).
use rackett::{
    builtin, cardigann,
    cardigann_exec::{template::Template, Plan},
    models::SearchQuery,
};
use serde_json::json;
use std::collections::BTreeMap;
fn plan(id: &str) -> Plan {
    let raw = builtin::SOURCES
        .iter()
        .find(|(name, _)| *name == id)
        .unwrap()
        .1;
    Plan::compile(&cardigann::parse(raw, cardigann::Limits::default()).unwrap()).unwrap()
}

#[test]
fn absent_ids_use_keyword_fallback_and_present_ids_keep_the_pinned_precedence() {
    for (id, imdb, douban, expected, area) in [
        ("13city", "", "", "Fixture", "0"),
        ("13city", "tt1234567", "", "tt1234567", "4"),
        ("13city", "", "7654321", "7654321", "1"),
        ("13city", "tt1234567", "7654321", "tt1234567", "4"),
        ("ourbits", "", "7654321", "7654321", "5"),
        ("hdhome", "", "", "Fixture", "0"),
        ("hdhome", "tt1234567", "", "tt1234567", "4"),
        ("hddolby", "", "", "Fixture", "0"),
        (
            "hddolby",
            "tt1234567",
            "7654321",
            "tt1234567 7654321 Fixture",
            "1",
        ),
    ] {
        let p = plan(id);
        let mut q = SearchQuery {
            params: BTreeMap::from([("q".into(), "Fixture".into())]),
        };
        if !imdb.is_empty() {
            q.params.insert("imdbid".into(), imdb.into());
        }
        if !douban.is_empty() {
            q.params.insert("doubanid".into(), douban.into());
        }
        let context = p.context(&BTreeMap::new(), &q, chrono::Utc::now());
        for request in p.build_requests(&context).unwrap() {
            let url = url::Url::parse(&request.url).unwrap();
            let params: BTreeMap<_, _> = url.query_pairs().collect();
            assert_eq!(
                params.get("search").map(|s| s.as_ref()),
                Some(expected),
                "{id}"
            );
            assert_eq!(
                params.get("search_area").map(|s| s.as_ref()),
                Some(area),
                "{id}"
            );
        }
    }
}

#[test]
fn every_optional_id_has_null_absence_but_nonempty_text_is_preserved() {
    let p = plan("13city");
    for (name, key) in [
        ("IMDBID", "imdbid"),
        ("IMDBIDShort", "imdbid"),
        ("TMDBID", "tmdbid"),
        ("TVDBID", "tvdbid"),
        ("TVRageID", "rid"),
        ("TVMazeID", "tvmazeid"),
        ("TraktID", "traktid"),
        ("DoubanID", "doubanid"),
    ] {
        let template=Template::compile(&format!("{{{{ if eq .Query.{name} .False }}}}keyword{{{{ else }}}}{{{{ .Query.{name} }}}}{{{{ end }}}}")).unwrap();
        for values in [BTreeMap::new(), BTreeMap::from([(key.into(), "".into())])] {
            let q = SearchQuery { params: values };
            let context = p.context(&BTreeMap::new(), &q, chrono::Utc::now());
            assert_eq!(template.render(&context).unwrap(), "keyword", "{name}");
        }
        let raw = if key == "imdbid" { "tt1234567" } else { "123" };
        let q = SearchQuery {
            params: BTreeMap::from([(key.into(), raw.into())]),
        };
        let context = p.context(&BTreeMap::new(), &q, chrono::Utc::now());
        assert_eq!(
            template.render(&context).unwrap(),
            if name == "IMDBIDShort" {
                "1234567"
            } else {
                raw
            }
        );
        assert_eq!(
            context.values[&format!(".Query.{name}")],
            json!(if name == "IMDBIDShort" {
                "1234567"
            } else {
                raw
            })
        );
    }
}
