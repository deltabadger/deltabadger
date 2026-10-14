#![cfg(unix)] // shells out to bin/rails and kills a process group
//! The two templates the Rails side of the suite copies instead of rebuilding: the recorders' empty databases
//! (script/rust/schema_template.rb) and the install rails_install hands to every test (common::install_template_for).
mod common;
use sha2::{Digest, Sha256};
use std::os::unix::process::CommandExt;
use std::path::Path;

fn digest(path: &Path) -> String { format!("{:x}", Sha256::digest(std::fs::read(path).unwrap())) }

#[test]
fn a_copied_schema_template_is_the_install_a_per_scenario_schema_load_gives() {
    let scratch = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    // Raises unless schema, rows, pragmas, sqlite_sequence and the next ids are the same.
    common::rails(scratch.path(), "development", &["runner", "script/rust/schema_template_check.rb", out.path().to_str().unwrap()]);
}

#[test]
fn changing_one_install_leaves_the_template_and_every_other_install_as_they_were() {
    let template = common::rails_install_template();
    let (a, b) = (common::rails_install(), common::rails_install());
    let before: Vec<(String, String)> = common::INSTALL_FILES.iter().map(|f| (digest(&template.join(f)), digest(&b.path().join(f)))).collect();
    for f in common::INSTALL_FILES {
        let c = rusqlite::Connection::open(a.path().join(f)).unwrap();
        c.execute_batch("CREATE TABLE written_by_one_test (x); INSERT INTO written_by_one_test VALUES (1);").unwrap();
    }
    let after: Vec<(String, String)> = common::INSTALL_FILES.iter().map(|f| (digest(&template.join(f)), digest(&b.path().join(f)))).collect();
    assert_eq!(before, after, "a write to one install reached the template or another install");
    let other = rusqlite::Connection::open(b.path().join("production.sqlite3")).unwrap();
    let seen: i64 = other.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'written_by_one_test'", [], |r| r.get(0)).unwrap();
    assert_eq!(seen, 0);
}

/// The child half of the next test: builds a template in TEMPLATE_BUILD_CHILD="<base>\n<run>", to be killed part-way.
#[test]
fn template_build_child() {
    let Ok(spec) = std::env::var("TEMPLATE_BUILD_CHILD") else { return };
    let (base, run) = spec.split_once('\n').unwrap();
    common::install_template_for(Path::new(base), run);
}

#[test]
fn a_template_build_killed_part_way_is_never_handed_out_and_the_next_build_replaces_it() {
    let base = tempfile::tempdir().unwrap();
    let run = "killed-run";
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "template_build_child", "--nocapture"])
        .env("TEMPLATE_BUILD_CHILD", format!("{}\n{run}", base.path().display()))
        .process_group(0) // its bin/rails dies with it
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .spawn().unwrap();
    let staging = |dir: &Path| std::fs::read_dir(dir).unwrap().flatten().any(|e| e.file_name().to_string_lossy().contains(".staging-"));
    let started = std::time::Instant::now();
    while !staging(base.path()) {
        assert!(started.elapsed() < std::time::Duration::from_secs(120), "the child never started a build");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let pgid = format!("-{}", child.id());
    assert!(std::process::Command::new("kill").args(["-KILL", "--", &pgid]).status().unwrap().success());
    child.wait().unwrap();
    let published = base.path().join(format!("rails-install-{run}"));
    assert!(!published.exists(), "the build finished before it was killed; nothing was tested");
    assert!(staging(base.path()), "the killed build left its staging directory");

    let template = common::install_template_for(base.path(), run);
    assert_eq!(template, published);
    assert!(!staging(base.path()), "the next build removed the killed one's staging directory");
    for f in common::INSTALL_FILES {
        let c = rusqlite::Connection::open_with_flags(template.join(f), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(c.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0)).unwrap(), "ok");
    }
}

#[test]
fn a_published_template_that_fails_its_check_is_rebuilt_not_handed_out() {
    let base = tempfile::tempdir().unwrap();
    let template = common::install_template_for(base.path(), "damaged-run");
    std::fs::write(template.join("production_queue.sqlite3"), b"not a database").unwrap();
    let again = common::install_template_for(base.path(), "damaged-run");
    assert_eq!(again, template);
    let c = rusqlite::Connection::open_with_flags(again.join("production_queue.sqlite3"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let tables: i64 = c.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'solid_queue_processes'", [], |r| r.get(0)).unwrap();
    assert_eq!(tables, 1, "the damaged queue database was handed out again");
}
