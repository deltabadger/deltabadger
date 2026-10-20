//! One venue's decision grid (plan VB §1.8): script/rust/decisions.rb builds and records it in Rails, Rust plays every
//! scenario on a pristine copy (deltabadger::parity::decide), and each is compared whole, unless a validator names it. A
//! validator asserts what the listed divergence is, on its own terms (Rails' side included, so a Rails that stops
//! diverging fails it); a name list alone would let anything else in that scenario differ unseen.
use super::decisions::{built_grid, copy_dir, r5_cleanup_expected, rails};
use serde_json::Value;
use std::path::Path;

/// A listed divergence: which scenarios it covers (by name), and what it asserts of them (name, Rust's scenario dir,
/// Rails' output, Rust's output).
pub struct Validator<'a> { pub names: &'a dyn Fn(&str) -> bool, pub check: &'a Check }
/// What a listed divergence asserts: (name, Rust's scenario dir, Rails' output, Rust's output).
pub type Check = dyn Fn(&str, &Path, &Value, &Value) -> Result<(), String>;

/// A validator for every scenario whose name ends in "-<suffix>" for one of `suffixes`.
pub fn ending<'a>(suffixes: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a { move |name| suffixes.iter().any(|s| name.ends_with(&format!("-{s}"))) }

/// Plays `grid` (a decisions.rb grid command) with `expected` scenarios: each compared whole, or by the first validator
/// naming it. Panics with every difference. Returns Rails' output per scenario, by name, for the caller's pins of what
/// Rails does where a silent pair of sides would otherwise pass.
pub async fn run_venue_grid(grid: &str, expected: usize, validators: &[Validator<'_>]) -> Vec<(String, Value)> {
    let rails_root = tempfile::tempdir().unwrap();
    let rust_root = tempfile::tempdir().unwrap();
    built_grid(grid, rails_root.path());
    let mut dirs: Vec<std::path::PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    assert_eq!(dirs.len(), expected, "{grid} has {} scenarios", dirs.len());
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); } // before Rails writes to its copies
    rails(&["record", rails_root.path().to_str().unwrap()]);
    let mut failures = vec![];
    let mut outputs = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        let rust_dir = rust_root.path().join(&name);
        let rust_out = deltabadger::parity::decide(&rust_dir).await.unwrap();
        let expected = r5_cleanup_expected(&rails_out, &rust_out);
        match validators.iter().find(|v| (v.names)(&name)) {
            Some(v) => if let Err(e) = (v.check)(&name, &rust_dir, &expected, &rust_out) { failures.push(format!("{name} (listed divergence): {e}")) },
            None if expected != rust_out => failures.push(format!("{name}\n  rails: {expected}\n  rust:  {rust_out}")),
            None => {}
        }
        outputs.push((name, rails_out));
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
    outputs
}
