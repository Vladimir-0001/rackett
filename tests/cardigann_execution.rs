use chrono::{TimeZone, Utc};
use rackett::{
    cardigann::{load, Limits},
    cardigann_exec::Plan,
    models::SearchQuery,
};
use std::{collections::BTreeMap, path::Path};
fn execute(kind: &str, q: &str) -> Vec<rackett::models::Release> {
    let node = load(
        Path::new(&format!("vendor/jackett/fixtures/{kind}-definition1.yml")),
        Limits::default(),
    )
    .unwrap();
    let plan = Plan::compile(&node).unwrap();
    let query = SearchQuery {
        params: BTreeMap::from([("q".into(), q.into()), ("t".into(), "search".into())]),
    };
    let context = plan.context(
        &BTreeMap::new(),
        &query,
        Utc.with_ymd_and_hms(2026, 10, 27, 12, 0, 0).unwrap(),
    );
    let requests = plan.build_requests(&context).unwrap();
    assert_eq!(requests.len(), 1);
    let expected = if kind == "html" {
        "https://www.testdefinition1.cc/search?query=ubuntu&sort=created"
    } else {
        "https://jsondefinition1.com/api/torrents/filter?api_token=&name=1080p&sortField=created_at&sortDirection=desc&perPage=100&page=1"
    };
    assert_eq!(requests[0].url, expected);
    let extension = if kind == "html" { "html" } else { "json" };
    let body = std::fs::read_to_string(format!(
        "vendor/jackett/fixtures/{kind}-response1.{extension}"
    ))
    .unwrap();
    plan.extract(&requests[0], &body, &context).unwrap()
}
#[test]
fn upstream_html_response_golden() {
    let releases = execute("html", "ubuntu");
    assert_eq!(releases.len(), 25);
    let r = &releases[0];
    assert_eq!(r.categories, [8000]);
    assert_eq!(r.title, "ubuntu-19.04-desktop-amd64.iso");
    assert_eq!(
        r.details.as_deref(),
        Some("https://www.testdefinition1.cc/torrent/d540fc48eb12f2833163eed6421d449dd8f1ce1f")
    );
    assert_eq!(
        r.link,
        "http://itorrents.org/torrent/d540fc48eb12f2833163eed6421d449dd8f1ce1f.torrent"
    );
    assert_eq!(r.guid, r.link);
    assert_eq!(r.attributes["magnet"].split("&tr").next().unwrap(),"magnet:?xt=urn:btih:d540fc48eb12f2833163eed6421d449dd8f1ce1f&dn=ubuntu-19.04-desktop-amd64.iso");
    assert_eq!(
        r.attributes["infohash"],
        "d540fc48eb12f2833163eed6421d449dd8f1ce1f"
    );
    assert!(r.published.as_deref().unwrap().starts_with("2024-"));
    assert_eq!(r.size, Some(2097152000));
    assert_eq!(r.seeders, Some(12));
    assert_eq!(r.peers, Some(13));
    assert_eq!(r.attributes["downloadvolumefactor"], "1");
    assert_eq!(r.attributes["uploadvolumefactor"], "2");
    assert_eq!(r.attributes["gain"], "23.4375");
}
#[test]
fn upstream_json_response_golden() {
    let releases = execute("json", "1080p");
    assert_eq!(releases.len(), 78);
    let r = &releases[0];
    assert_eq!(r.categories, [2000, 100001]);
    assert_eq!(
        r.title,
        "The Eyes of Tammy Faye (2021)  BDRip 1080p AVC ES DD+ 5.1 EN DTSSS 5.1 Subs] HDO"
    );
    assert_eq!(
        r.details.as_deref(),
        Some("https://jsondefinition1.com/torrents/24804")
    );
    assert_eq!(
        r.link,
        "https://jsondefinition1.com/torrent/download/24804.01c887e14d0845f195bc12b31ea27d38"
    );
    assert_eq!(r.guid, r.link);
    assert!(!r.attributes.contains_key("magnet"));
    assert!(!r.attributes.contains_key("infohash"));
    assert_eq!(
        r.attributes["poster"],
        "https://image.tmdb.org/t/p/w92/iBjkm6oxTPrvNkzr63cmnrpsQPR.jpg"
    );
    assert!(r.published.as_deref().unwrap().starts_with("2021-"));
    assert_eq!(r.size, Some(17964744495));
    assert_eq!(r.seeders, Some(27));
    assert_eq!(r.peers, Some(30));
    for (k, v) in [
        ("files", "1"),
        ("grabs", "29"),
        ("downloadvolumefactor", "1"),
        ("uploadvolumefactor", "1"),
        ("minimumseedtime", "345600"),
        ("imdbid", "9115530"),
        ("tmdbid", "601470"),
        ("tvdbid", "0"),
    ] {
        assert_eq!(r.attributes[k], v);
    }
    assert!(!r.attributes.contains_key("minimumratio"));
    assert!(!r.attributes.contains_key("rageid"));
    assert!((r.attributes["gain"].parse::<f64>().unwrap() - 451.7362465756014).abs() < 1e-12);
}
fn synthetic(fields: &str) -> rackett::cardigann::Node {
    rackett::cardigann::parse(&format!("id: synthetic\nname: Synthetic\ndescription: Test\nlanguage: en-US\ntype: public\nencoding: UTF-8\nlinks: [https://example.org/]\ncaps:\n  categories: {{Other: Other}}\n  modes: {{search: [q]}}\nsearch:\n  paths: [{{path: search}}]\n  rows: {{selector: article}}\n  fields:\n    category: {{text: Other}}\n    seeders: {{text: 0}}\n    date: {{text: 2026-10-27T12:00:00Z}}\n    size: {{text: 0}}\n{fields}"),Limits::default()).unwrap()
}
#[test]
fn ordered_fields_optional_defaults_and_isolated_context() {
    let node=synthetic("    title: {selector: h1}\n    _part: {text: ' suffix'}\n    title|append: {text: '{{ .Result._part }}'}\n    size:\n      selector: .missing\n      optional: true\n      default: '12'\n    details: {text: '/details'}\n    download: {text: '/download'}");
    let plan = std::sync::Arc::new(Plan::compile(&node).unwrap());
    let ctx = plan.context(&BTreeMap::new(), &SearchQuery::default(), Utc::now());
    let request = plan.build_requests(&ctx).unwrap().remove(0);
    let handles = (0..4)
        .map(|n| {
            let plan = plan.clone();
            let ctx = ctx.clone();
            let request = request.clone();
            std::thread::spawn(move || {
                let releases = plan
                    .extract(
                        &request,
                        &format!("<article><h1>Release{n}</h1></article>"),
                        &ctx,
                    )
                    .unwrap();
                assert_eq!(releases[0].title, format!("Release{n} suffix"));
                assert_eq!(releases[0].size, Some(12));
            })
        })
        .collect::<Vec<_>>();
    for h in handles {
        h.join().unwrap();
    }
    assert!(!ctx.values.keys().any(|k| k.starts_with(".Result.")));
}
#[test]
fn unsupported_semantics_and_response_limits_are_explicit() {
    let node = synthetic("    title: {text: title}\n    download: {text: '/d'}");
    let plan = Plan::compile(&node).unwrap();
    let ctx = plan.context(&BTreeMap::new(), &SearchQuery::default(), Utc::now());
    let request = plan.build_requests(&ctx).unwrap().remove(0);
    assert!(plan
        .extract(&request, &"<article></article>".repeat(1001), &ctx)
        .unwrap_err()
        .message
        .contains("limit"));
    assert!(plan
        .extract(&request, &"x".repeat(8 * 1024 * 1024 + 1), &ctx)
        .unwrap_err()
        .message
        .contains("budget"));
    assert!(Plan::compile(&synthetic(
        "    title: {selector: h1, remove: 'span:unknown-extension(x)'}\n    download: {text: '/d'}"
    ))
    .is_err());
    assert!(Plan::compile(&synthetic(
        "    title: {text: '{{ unsupported .Query.Q }}'}\n    download: {text: '/d'}"
    ))
    .is_err());
}
