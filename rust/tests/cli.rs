use std::process::Command;

#[test]
fn resolve_placement_refuses_a_database_url_before_touching_any_database() {
    for var in ["DATABASE_URL", "PRIMARY_DATABASE_URL", "QUEUE_DATABASE_URL"] {
        let dir = tempfile::tempdir().unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_deltabadger"))
            .args(["resolve-placement", "1", "--not-placed"])
            .env("STORAGE_DIR", dir.path())
            .env(var, "sqlite3:/elsewhere.sqlite3")
            .output()
            .unwrap();
        assert!(!out.status.success(), "{var}: must exit non-zero");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(var) && err.contains("*_DATABASE_PATH"), "{var}: unclear message: {err}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "{var}: no lock or database file may be created");
    }
}
