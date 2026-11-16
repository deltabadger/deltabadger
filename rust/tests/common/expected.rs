//! D-B11-EXPECTED-OWNED (PLAN-B11 §2): an answer frozen from Rails is the Rust suite's own expected value. When Rust's
//! output changes on purpose, `EXPECTED_REWRITE=1` rewrites the expected file from it and fails the test, so the diff is
//! reviewed and committed; CI never sets it.
use std::path::Path;

pub fn rewriting() -> bool { std::env::var("EXPECTED_REWRITE").is_ok_and(|v| v == "1") }

/// Writes `text` to `path` and fails the test.
pub fn rewrite(path: &Path, text: &[u8]) -> ! {
    std::fs::write(path, text).unwrap();
    panic!("EXPECTED_REWRITE=1: rewrote {}; review and commit it, then run without EXPECTED_REWRITE", path.display())
}
