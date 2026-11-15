-- Test-only (tests/rust_migrations.rs): the two kinds of replacement (D-B11-9-DROP-OWN-TABLE).
-- (a) A rename: the new table, every row copied with its rowid, the old table dropped.
CREATE TABLE rust_fx_new (name TEXT NOT NULL, at TEXT);
INSERT INTO rust_fx_new (rowid, name, at) SELECT rowid, name, at FROM rust_fx_old;
DROP TABLE rust_fx_old;
-- (b) A rebuild under the same name, through a hop table: a value and its CHECK change, label becomes nullable, its
-- unique index goes, and a column is added.
CREATE TABLE rust_fx_kept_hop (r INTEGER PRIMARY KEY, label TEXT, kind TEXT NOT NULL);
INSERT INTO rust_fx_kept_hop (r, label, kind) SELECT rowid, label, CASE kind WHEN 'b' THEN 'c' ELSE kind END FROM rust_fx_kept;
DROP TABLE rust_fx_kept;
CREATE TABLE rust_fx_kept (id INTEGER PRIMARY KEY, label TEXT, kind TEXT NOT NULL CHECK(kind IN ('a', 'c')), note TEXT NOT NULL DEFAULT '');
INSERT INTO rust_fx_kept (id, label, kind) SELECT r, label, kind FROM rust_fx_kept_hop;
DROP TABLE rust_fx_kept_hop;
