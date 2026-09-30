//! Embeds db/sql/migrate/*.sql (the SQL twins of Rails migrations) in version order.
use std::{env, fs, path::PathBuf};

fn main() {
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../db/sql/migrate");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut twins: Vec<(String, PathBuf)> = fs::read_dir(&dir)
        .map(|entries| entries.filter_map(Result::ok).map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "sql"))
            .map(|p| (p.file_name().unwrap().to_string_lossy()[..14].to_string(), p))
            .collect())
        .unwrap_or_default();
    twins.sort();
    let body: String = twins.iter()
        .map(|(v, p)| format!("    ({v:?}, include_str!({:?})),\n", p.canonicalize().unwrap()))
        .collect();
    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("twins.rs");
    fs::write(out, format!("pub const TWINS: &[(&str, &str)] = &[\n{body}];\n")).unwrap();
}
