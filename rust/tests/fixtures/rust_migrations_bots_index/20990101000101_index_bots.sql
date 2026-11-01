-- Test-only (tests/schema_baseline.rs): a Rust migration adding an index to a Rails-born table, which must be caught.
CREATE INDEX probe_index_on_bots ON bots (id);
