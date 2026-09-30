//! Records the Rails migration versions this build understands.
use std::{env, fs, path::PathBuf};

fn main() {
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../db/migrate");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut versions = Vec::new();
    for entry in fs::read_dir(&dir).expect("read Rails migrations") {
        let entry = entry.expect("read migration entry");
        if !entry
            .file_type()
            .expect("read migration file type")
            .is_file()
        {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_str().expect("migration filename is UTF-8");
        let version = name.get(..14).expect("migration has a version prefix");
        assert!(
            version.bytes().all(|b| b.is_ascii_digit()),
            "invalid migration version: {name}"
        );
        versions.push(version.to_owned());
    }
    versions.sort();
    let body: String = versions.iter().map(|v| format!("    {v:?},\n")).collect();
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("migrations.rs");
    fs::write(
        out,
        format!("pub const MIGRATIONS: &[&str] = &[\n{body}];\n"),
    )
    .unwrap();
}
