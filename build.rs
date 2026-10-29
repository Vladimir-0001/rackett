use std::{env, fs, path::PathBuf};
fn main() {
    let dir = PathBuf::from("vendor/jackett/definitions");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "yml"))
        .collect();
    files.sort_by(|a, b| a.file_stem().cmp(&b.file_stem()));
    assert_eq!(
        files.len(),
        583,
        "Pinned catalog changed; review provenance"
    );
    let mut out = String::from("pub static SOURCES: &[(&str, &str)] = &[\n");
    for p in files {
        let id = p.file_stem().unwrap().to_str().unwrap();
        out.push_str(&format!("({id:?}, include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/vendor/jackett/definitions/{id}.yml\"))),\n"));
    }
    out.push_str("];\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("builtin_sources.rs"),
        out,
    )
    .unwrap();
}
