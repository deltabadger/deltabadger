-- Test-only (tests/schema_baseline.rs): a Rust migration adding a trigger to a Rails-born table, which must be caught.
CREATE TRIGGER probe_trigger_on_bots AFTER INSERT ON bots BEGIN SELECT 1; END;
