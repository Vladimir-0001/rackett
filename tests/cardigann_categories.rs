//! Derived from pinned TorznabCapabilitiesCategories.cs and its NUnit tests,
//! plus adversarial cases exercising the same ordered mapping algorithm.
use chrono::{TimeZone, Utc};
use rackett::{
    cardigann::{parse, Limits},
    cardigann_exec::{template::Context, Plan},
    models::SearchQuery,
};
use std::collections::BTreeMap;
fn plan(caps: &str, fields: &str) -> Result<Plan, rackett::cardigann_exec::ExecutionError> {
    let node = parse(&format!("id: categories\nname: Categories\ndescription: Test\nlanguage: en-US\ntype: public\nencoding: UTF-8\nlinks: [https://example.org/]\ncaps:\n{caps}\n  modes: {{search: [q]}}\nsearch:\n  paths: [{{path: search}}]\n  inputs: {{cats: '{{{{ join .Categories \",\" }}}}'}}\n  rows: {{selector: article}}\n  fields:\n    title: {{text: Test}}\n    seeders: {{text: 0}}\n    size: {{text: 0}}\n    date: {{text: 2026-10-27T12:00:00Z}}\n    download: {{text: 'https://example.org/file'}}\n{fields}"), Limits::default()).unwrap();
    Plan::compile(&node)
}
fn context(p: &Plan, cat: &str) -> Context {
    p.context(
        &BTreeMap::new(),
        &SearchQuery {
            params: BTreeMap::from([("cat".into(), cat.into())]),
        },
        Utc.with_ymd_and_hms(2026, 10, 27, 12, 0, 0).unwrap(),
    )
}
fn mapped(p: &Plan, cat: &str) -> Vec<String> {
    let ctx = context(p, cat);
    // Exercise the normal request-building path too.
    p.build_requests(&ctx).unwrap();
    ctx.values[".Categories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect()
}
fn result(p: &Plan) -> Vec<u32> {
    let ctx = context(p, "");
    let req = p.build_requests(&ctx).unwrap();
    p.extract(&req[0], "<article></article>", &ctx).unwrap()[0]
        .categories
        .clone()
}
const DATA: &str = "  categorymappings:\n    - {id: '1', cat: Movies}\n    - {id: mov_sd, cat: Movies/SD}\n    - {id: '33', cat: Books/Comics}\n    - {id: '44', cat: Console/XBox, desc: Console/Xbox_c}\n    - {id: con_wii, cat: Console/Wii, desc: Console/Wii_c}\n    - {id: '40', cat: Console/XBox, desc: Console/Xbox_c2}";
#[test]
fn upstream_mixed_dataset_queries_and_normalized_results() {
    let p = plan(DATA, "    category: {text: CON_WII}").unwrap();
    for (query, expected) in [
        ("2000", vec!["1", "mov_sd"]),
        ("2030", vec!["mov_sd"]),
        ("1040", vec!["44", "40"]),
        ("100044", vec!["44"]),
        ("137107", vec!["con_wii"]),
        ("1000", vec!["44", "con_wii", "40"]),
        ("2030,2000,2030", vec!["1", "mov_sd"]),
        ("9999", vec![]),
        ("100001", vec![]),
        ("", vec![]),
    ] {
        assert_eq!(mapped(&p, query), expected, "query {query}");
    }
    assert_eq!(result(&p), [1030, 137107]);
    assert_eq!(p.supported_categories(), [1030, 1040, 2000, 2030, 7030]);
    for (field, expected) in [
        ("category: {text: '44'}", vec![1040, 100044]),
        ("category: {text: mov_sd}", vec![2030]),
        ("categorydesc: {text: console/wii_C}", vec![1030, 137107]),
        ("category: {text: unknown}", vec![]),
    ] {
        assert_eq!(
            result(&plan(DATA, &format!("    {field}")).unwrap()),
            expected
        );
    }
}
#[test]
fn numeric_lexemes_and_sha1_are_exact() {
    for (id, custom) in [
        ("44", 100044),
        ("01", 100001),
        (" +01 ", 100001),
        ("-1", 99999),
        ("-100000", 0),
        ("2147383647", 2147483647),
        ("2147483648", 134421),
        ("4294967295", 106429),
        ("con_wii", 137107),
        ("Movies", 100467),
        ("movies", 112696),
    ] {
        let caps = format!("  categorymappings:\n    - {{id: '{id}', cat: Other, desc: Custom}}");
        let p = plan(&caps, &format!("    category: {{text: '{id}'}}")).unwrap();
        assert_eq!(result(&p), [8000, custom], "id {id}");
        assert_eq!(mapped(&p, &custom.to_string()), [id], "query id {id}");
    }
    for id in ["-100001", "2147383648", "2147483647", "-2147483648"] {
        assert!(plan(
            &format!("  categorymappings:\n    - {{id: '{id}', cat: Other, desc: Custom}}"),
            "    category: {text: Other}"
        )
        .is_err());
    }
}
#[test]
fn collisions_keep_both_mappings_and_numeric_string_collisions() {
    // Independently reproducible SHA1 prefixes: collision_497 and collision_511
    // both start 4d b7; UInt16 little endian = 46925. No probing or reassignment.
    let caps="  categorymappings:\n    - {id: collision_497, cat: Movies/HD, desc: A}\n    - {id: collision_511, cat: TV/HD, desc: B}\n    - {id: '46925', cat: Books, desc: C}";
    let p = plan(caps, "    category: {text: collision_511}").unwrap();
    assert_eq!(
        mapped(&p, "146925"),
        ["collision_497", "collision_511", "46925"]
    );
    assert_eq!(result(&p), [5040, 146925]);
    assert_eq!(
        result(&plan(caps, "    categorydesc: {text: A}").unwrap()),
        [2040, 146925]
    );
}
#[test]
fn duplicate_mapping_noappend_and_ordered_union() {
    let caps="  categorymappings:\n    - {id: same, cat: Movies, desc: Same, default: true}\n    - {id: same, cat: TV, desc: Same, default: true}";
    // SHA1('same') first UInt16 = 13311.
    let initial = result(&plan(caps, "    category: {text: same}").unwrap());
    assert_eq!(initial, [2000, 113311, 5000]);
    assert_eq!(
        mapped(
            &plan(caps, "    category: {text: same}").unwrap(),
            "2000,5000"
        ),
        ["same"]
    );
    assert_eq!(
        mapped(&plan(caps, "    category: {text: same}").unwrap(), "9999"),
        ["same", "same"]
    ); // defaults preserve duplicates
    assert_eq!(
        result(
            &plan(
                caps,
                "    category: {text: same}\n    category|append: {text: same}"
            )
            .unwrap()
        ),
        [2000, 113311, 5000]
    );
    assert_eq!(
        result(
            &plan(
                caps,
                "    category: {text: same}\n    category|noappend: {text: same}"
            )
            .unwrap()
        ),
        [2000, 113311, 5000, 113311]
    );
    assert_eq!(
        result(&plan(caps, "    categorydesc: {text: Same}").unwrap()),
        initial
    );
    assert_eq!(
        result(
            &plan(
                caps,
                "    categorydesc: {text: Same}\n    categorydesc|noappend: {text: Same}"
            )
            .unwrap()
        ),
        [2000, 113311, 5000, 113311]
    );
}
#[test]
fn standard_id_collision_uses_real_hierarchy_only() {
    let caps="  categorymappings:\n    - {id: '-97970', cat: TV, desc: ChildCollision}\n    - {id: '-97901', cat: Books, desc: UnknownRoot}";
    let p = plan(caps, "    category: {text: '-97970'}").unwrap();
    assert_eq!(mapped(&p, "2000"), ["-97970"]); // custom 2030 is real Movies/SD
    assert_eq!(mapped(&p, "2099"), ["-97901"]); // custom 2099 is not a Movies child
    assert_eq!(mapped(&p, "2030"), ["-97970"]);
}
#[test]
fn bounded_and_explicit_unsupported_comparison() {
    let p = plan(
        "  categorymappings:\n    - {id: 'é', cat: Other, desc: Unicode}",
        "    category: {text: 'é'}",
    )
    .unwrap();
    assert_eq!(mapped(&p, "105567"), ["é"]);
    assert_eq!(result(&p), [8000, 105567]);
    let p = plan("  categories: {'é': Other}", "    category: {text: ß}").unwrap();
    let ctx = context(&p, "");
    let req = p.build_requests(&ctx).unwrap();
    assert!(p
        .extract(&req[0], "<article></article>", &ctx)
        .unwrap_err()
        .message
        .contains("category collation"));
    assert!(plan(
        &format!("  categories: {{'{}': Other}}", "a".repeat(4097)),
        "    category: {text: Other}"
    )
    .err()
    .unwrap()
    .message
    .contains("text limit"));
    let caps = format!(
        "  categorymappings:\n{}",
        (0..4097)
            .map(|i| format!("    - {{id: '{i}', cat: Other}}\n"))
            .collect::<String>()
    );
    assert!(plan(&caps, "    category: {text: Other}")
        .err()
        .unwrap()
        .message
        .contains("mapping limit"));
}
#[test]
fn description_presence_whitespace_and_accent_rules() {
    let caps="  categorymappings:\n    - {id: con_wii, cat: Console/Wii}\n    - {id: '44', cat: Console/XBox, desc: ''}\n    - {id: '40', cat: Movies, desc: Películas}";
    assert_eq!(
        result(&plan(caps, "    category: {text: con_wii}").unwrap()),
        [1030]
    );
    assert_eq!(
        mapped(
            &plan(caps, "    category: {text: con_wii}").unwrap(),
            "100044"
        ),
        ["44"]
    );
    for (value, expected) in [
        ("PELÍCULAS", vec![2000, 100040]),
        ("Películas", vec![2000, 100040]),
        ("Peliculas", vec![]),
        (" Películas ", vec![]),
        (" ", vec![]),
        ("", vec![]),
    ] {
        assert_eq!(
            result(&plan(caps, &format!("    categorydesc: {{text: '{value}'}}")).unwrap()),
            expected
        );
    }
    assert_eq!(
        result(
            &plan(
                caps,
                "    category: {text: con_wii}\n    category|noappend: {text: unknown}"
            )
            .unwrap()
        ),
        [1030]
    );
}
#[test]
fn aggregate_category_text_and_extraction_work_are_bounded() {
    let caps = format!(
        "  categorymappings:\n{}",
        (0..257)
            .map(|i| format!(
                "    - {{id: '{i}', cat: Other, desc: '{}'}}\n",
                "x".repeat(4096)
            ))
            .collect::<String>()
    );
    assert!(plan(&caps, "    category: {text: Other}")
        .err()
        .unwrap()
        .message
        .contains("text limit"));
    let caps = format!(
        "  categorymappings:\n{}",
        (0..4096)
            .map(|i| format!("    - {{id: '{i}', cat: Other}}\n"))
            .collect::<String>()
    );
    let fields = format!(
        "    category: {{text: unknown}}\n{}",
        "    category|optional: {text: '0'}\n".repeat(4)
    );
    let p = plan(&caps, &fields).unwrap();
    let ctx = context(&p, "");
    let req = p.build_requests(&ctx).unwrap();
    // Category lookup operations count even on optional fields, and budget errors
    // cannot be swallowed by optional-field handling. The fifth lookup is the
    // final optional field on the only row: silently skipping it would succeed.
    let err = p.extract(&req[0], "<article></article>", &ctx).unwrap_err();
    assert_eq!(err.path, "$.response");
    assert!(err.message.contains("operation budget"));
}

#[test]
fn exact_unsupported_collation_still_rejects() {
    for id in ["ß", "x\u{200d}y"] {
        let p = plan(
            &format!("  categories: {{'{id}': Other}}"),
            &format!("    category: {{text: '{id}'}}"),
        )
        .unwrap();
        let ctx = context(&p, "");
        let req = p.build_requests(&ctx).unwrap();
        assert!(p
            .extract(&req[0], "<article></article>", &ctx)
            .unwrap_err()
            .message
            .contains("category collation"));
    }
}
