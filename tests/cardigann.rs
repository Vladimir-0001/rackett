use rackett::cardigann::*;
use std::path::Path;

fn parse_ok(s: &str) -> Node {
    parse(s, Limits::default()).unwrap()
}
#[test]
fn ordered_repeated_fields_and_scalars_are_lossless() {
    let root = parse_ok("search:\n  fields:\n    title: {text: first}\n    _temporary: {text: 01}\n    title: {text: '{{ .Result._temporary }}'}\n    title|append|optional: {text: yes}\n    category|noappend: {text: null}\nvalues: [01, 0x10, yes, on, null, ~, 2026-10-27, 1e3, 'false', \"02\"]\n");
    let search = Definition { raw: &root }.search().unwrap();
    let fields: Vec<_> = search.fields().collect();
    assert_eq!(
        fields.iter().map(|f| f.name).collect::<Vec<_>>(),
        [
            "title",
            "_temporary",
            "title",
            "title|append|optional",
            "category|noappend"
        ]
    );
    assert_eq!(fields[1].selector.text(), Some("01"));
    assert!(fields[1].intermediate());
    assert_eq!(fields[3].base_name(), "title");
    assert_eq!(
        fields[3].modifiers().collect::<Vec<_>>(),
        ["append", "optional"]
    );
    assert_eq!(
        root.get("values")
            .unwrap()
            .items()
            .unwrap()
            .iter()
            .map(|n| n.text().unwrap())
            .collect::<Vec<_>>(),
        [
            "01",
            "0x10",
            "yes",
            "on",
            "null",
            "~",
            "2026-10-27",
            "1e3",
            "false",
            "02"
        ]
    );
    assert_eq!(
        root.get("search")
            .unwrap()
            .get("fields")
            .unwrap()
            .all("title")
            .count(),
        2
    );
}
#[test]
fn invalid_and_dangerous_yaml_is_explicitly_rejected() {
    for (input, code) in [
        ("a: &a [one]", "yaml_anchor_or_tag"),
        ("a: *missing", "yaml_syntax"),
        ("a: !!str 01", "yaml_anchor_or_tag"),
        ("<<: {a: b}", "yaml_merge"),
        ("? [a,b]\n: c", "complex_key"),
        ("---\na: b\n---\nc: d", "yaml_documents"),
        ("a: [broken", "yaml_syntax"),
        ("%YAML 1.2\n---\na: b", "yaml_directive"),
    ] {
        assert_eq!(
            parse(input, Limits::default()).unwrap_err().code,
            code,
            "{input}"
        );
    }
}
#[test]
fn bounded_parser_does_not_panic() {
    let source = "a: [b,c]";
    assert_eq!(
        parse(
            source,
            Limits {
                bytes: 2,
                ..Limits::default()
            }
        )
        .unwrap_err()
        .code,
        "byte_limit"
    );
    assert_eq!(
        parse(
            source,
            Limits {
                nodes: 2,
                ..Limits::default()
            }
        )
        .unwrap_err()
        .code,
        "node_limit"
    );
    let deep = format!("{}x{}", "[".repeat(100), "]".repeat(100));
    assert_eq!(
        parse(&deep, Limits::default()).unwrap_err().code,
        "depth_limit"
    );
    for n in 0..source.len() {
        let _ = parse(&source[..n], Limits::default());
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.yml");
    std::fs::write(&path, source).unwrap();
    assert_eq!(
        load(
            &path,
            Limits {
                bytes: 2,
                ..Limits::default()
            }
        )
        .unwrap_err()
        .code,
        "byte_limit"
    );
    std::fs::write(&path, [0xff, 0xfe]).unwrap();
    assert_eq!(load(&path, Limits::default()).unwrap_err().code, "encoding");
}
#[test]
fn all_583_pinned_definitions_validate_offline() {
    let report = check_path(Path::new("vendor/jackett/definitions"), Limits::default()).unwrap();
    let failures: Vec<_> = report
        .definitions
        .iter()
        .filter(|d| !d.model_valid)
        .collect();
    assert!(failures.is_empty(), "{failures:#?}");
    assert_eq!(report.summary.total, 583);
    assert_eq!(report.summary.executable, 0);
    assert!(report.definitions.iter().all(|d| !d.execution_supported));
    assert!(report
        .summary
        .required_features
        .contains("filter:dateparse"));
    let root = load(
        Path::new("vendor/jackett/definitions/btarg.yml"),
        Limits::default(),
    )
    .unwrap();
    let caps = Definition { raw: &root }.caps().unwrap();
    assert!(
        caps.category_mappings()
            .iter()
            .any(|n| n.string("id") == Some("01"))
            || caps
                .categories()
                .iter()
                .any(|(k, _)| k.text() == Some("01"))
    );
}
#[test]
fn upstream_html_and_json_model_fixtures() {
    for (file, id, language) in [
        ("html-definition1.yml", "testdefinition1", "en-US"),
        ("json-definition1.yml", "jsondefinition1", "es-ES"),
    ] {
        let root = load(
            &Path::new("vendor/jackett/fixtures").join(file),
            Limits::default(),
        )
        .unwrap();
        assert!(validate(&root)
            .iter()
            .all(|d| d.severity != Severity::Error));
        let definition = Definition { raw: &root };
        assert_eq!(definition.metadata().id(), Some(id));
        assert_eq!(definition.metadata().language(), Some(language));
        assert!(definition.metadata().test_link_torrent());
        assert!(!definition.metadata().follow_redirect());
        assert_eq!(definition.caps().unwrap().modes().len(), 5);
        assert_eq!(definition.search().unwrap().paths().len(), 1);
        assert!(definition
            .search()
            .unwrap()
            .fields()
            .any(|f| f.name == "seeders"));
        if file.starts_with("json") {
            let settings: Vec<_> = definition.settings().collect();
            assert_eq!(
                settings
                    .iter()
                    .find(|s| s.name() == Some("freeleech"))
                    .unwrap()
                    .default(),
                Some("false")
            );
            assert_eq!(definition.login().unwrap().method(), Some("get"));
            let cats = definition.caps().unwrap();
            assert_eq!(cats.category_mappings()[0].string("id"), Some("1"));
        }
    }
}
#[test]
fn raw_unknown_nodes_and_id_aliases_survive() {
    let root = parse_ok(
        "id: example\nreplaces: [old-id, 01]\nfuture: {new: [a, b]}\nratio: {path: /ratio}\n",
    );
    assert_eq!(
        root.get("replaces").unwrap().items().unwrap()[1].text(),
        Some("01")
    );
    assert_eq!(
        root.get("future")
            .unwrap()
            .get("new")
            .unwrap()
            .items()
            .unwrap()
            .len(),
        2
    );
    assert!(validate(&root)
        .iter()
        .any(|d| d.code == "unknown_property" && d.path == "$.future"));
    assert!(validate(&root).iter().any(|d| d.code == "model_required"));
}
#[test]
fn cli_is_deterministic_and_creates_no_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/jackett/fixtures");
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_rackett"))
            .args(["definitions", "check"])
            .arg(&path)
            .arg("--json")
            .current_dir(dir.path())
            .env("RACKETT_DATA_DIR", dir.path().join("never-created"))
            .output()
            .unwrap()
    };
    let first = run();
    let second = run();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(first.stdout, second.stdout);
    let report: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(report["summary"]["total"], 2);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
#[test]
fn duplicate_catalog_ids_are_errors_for_both_files() {
    let dir = tempfile::tempdir().unwrap();
    let source = std::fs::read("vendor/jackett/fixtures/html-definition1.yml").unwrap();
    for name in ["a.yml", "b.yml"] {
        std::fs::write(dir.path().join(name), &source).unwrap();
    }
    let report = check_path(dir.path(), Limits::default()).unwrap();
    assert_eq!(report.summary.invalid, 2);
    assert!(report
        .definitions
        .iter()
        .all(|d| d.diagnostics.iter().any(|d| d.code == "duplicate_id")));
}

#[test]
fn validation_rejects_nested_shape_errors_and_missing_field_requirements() {
    let source = std::fs::read_to_string("vendor/jackett/fixtures/html-definition1.yml").unwrap();
    for changed in [
        source.replace("type: public", "type: nonsense"),
        source.replace("seeders:", "_seeders:"),
        source.replace(
            "text: Other",
            "filters: [{name: replace, args: [{bad: mapping}]}]",
        ),
    ] {
        assert!(validate(&parse_ok(&changed))
            .iter()
            .any(|d| d.severity == Severity::Error));
    }
    let legacy = parse_ok("search: {path: /browse, fields: {}}\n");
    let search = Definition { raw: &legacy }.search().unwrap();
    let requests: Vec<_> = search.requests().collect();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path(), Some("/browse"));
    assert!(requests[0].inherit_inputs());
    assert_eq!(requests[0].query_separator(), "&");
    assert_eq!(
        parse("\u{feff}%YAML 1.2\n---\na: b", Limits::default())
            .unwrap_err()
            .code,
        "yaml_directive"
    );
    assert_eq!(
        parse(
            "a: b",
            Limits {
                depth: 10000,
                ..Limits::default()
            }
        )
        .unwrap_err()
        .code,
        "limit_configuration"
    );
}

#[test]
fn cli_invalid_and_empty_checks_have_failure_exit_status() {
    let dir = tempfile::tempdir().unwrap();
    let run = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_rackett"))
            .args(["definitions", "check"])
            .arg(dir.path())
            .arg("--json")
            .output()
            .unwrap()
    };
    let empty = run();
    assert!(!empty.status.success());
    let json: serde_json::Value = serde_json::from_slice(&empty.stdout).unwrap();
    assert_eq!(json["summary"]["total"], 0);
    std::fs::write(dir.path().join("invalid.yml"), "a: [broken").unwrap();
    let invalid = run();
    assert!(!invalid.status.success());
    let json: serde_json::Value = serde_json::from_slice(&invalid.stdout).unwrap();
    assert_eq!(json["summary"]["invalid"], 1);
}

#[test]
fn pinned_vendor_inputs_match_checksums() {
    use sha2::{Digest, Sha256};
    let manifest = std::fs::read_to_string("vendor/jackett/SHA256SUMS").unwrap();
    let mut definitions = 0;
    for line in manifest.lines() {
        let (expected, path) = line.split_once("  ").unwrap();
        let bytes = std::fs::read(Path::new("vendor/jackett").join(path)).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(bytes)), expected, "{path}");
        if path.starts_with("definitions/") {
            definitions += 1;
        }
    }
    assert_eq!(definitions, 583);
}
