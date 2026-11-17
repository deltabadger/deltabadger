-- Test-only (tests/schema_baseline.rs): a Rust migration adding an index to a 2.x-born table, which must be caught.
CREATE INDEX probe_index_on_bots ON bots (id);
