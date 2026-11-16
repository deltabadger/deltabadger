-- Test-only (tests/rust_migrations.rs): two Rust tables a later migration replaces.
CREATE TABLE rust_fx_old (name TEXT NOT NULL, at TEXT);
CREATE TABLE rust_fx_kept (id INTEGER PRIMARY KEY, label TEXT NOT NULL, kind TEXT NOT NULL CHECK(kind IN ('a', 'b')));
CREATE UNIQUE INDEX index_rust_fx_kept_on_label ON rust_fx_kept (label);
