//! The 3.0 image's entrypoint (docker/entrypoint.sh) provisions /app/storage/.secrets exactly as the 2.x image's
//! (docker-entrypoint.sh) does (PLAN-B3 §2): its secrets block is a copy, checked line for line below, and the cases of
//! test/config/docker_entrypoint_secrets_test.rb run against it here, under bash, without Docker or Ruby.
#![cfg(unix)]

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PRIMARY: &str = "ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY";
const SALT: &str = "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT";
const MARKER: &str = "ACTIVE_RECORD_ENCRYPTION_KEYS_EXTERNAL";

fn root() -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("..") }
fn script() -> PathBuf { root().join("docker/entrypoint.sh") }

/// Sources the script without running main, then runs `body`, with only the variables named here set.
fn bash(dir: &Path, env: &[(&str, &str)], body: &str) -> Output {
    let mut c = Command::new("bash");
    c.arg("-c").arg(format!("DELTABADGER_ENTRYPOINT_SOURCED=1\nexport DELTABADGER_ENTRYPOINT_SOURCED\n. \"{}\"\n{body}", script().display()));
    for v in ["DATABASE_URL", "PRIMARY_DATABASE_URL", "SECRET_KEY_BASE", PRIMARY, SALT, MARKER, "APP_ENCRYPTION_KEY", "DEVISE_SECRET_KEY"] { c.env_remove(v); }
    c.env("SECRETS_FILE", dir.join(".secrets")).env("DATABASE_PATH", dir.join("production.sqlite3"));
    for (k, v) in env { c.env(k, v); }
    c.output().expect("bash runs")
}

/// setup_secrets, and what the app process would see in its environment.
fn run_setup(dir: &Path, env: &[(&str, &str)]) -> Output {
    bash(dir, env, &format!("setup_secrets 1>&2 || exit 1\necho \"SECRET_KEY_BASE=${{SECRET_KEY_BASE}}\"\n\
        echo \"{PRIMARY}=${{{PRIMARY}}}\"\necho \"{SALT}=${{{SALT}}}\"\necho \"{MARKER}=${{{MARKER}}}\""))
}

fn provision(dir: &Path, env: &[(&str, &str)]) -> HashMap<String, String> {
    let out = run_setup(dir, env);
    assert!(out.status.success(), "entrypoint failed: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn secrets_file(dir: &Path) -> String { std::fs::read_to_string(dir.join(".secrets")).unwrap() }

#[test]
fn the_secrets_block_is_the_2x_entrypoints_but_for_one_comment_line() {
    let old = std::fs::read_to_string(root().join("docker-entrypoint.sh")).unwrap();
    let new = std::fs::read_to_string(script()).unwrap();
    // From the SECRETS_FILE line to the end of setup_secrets, in each file.
    let block = |text: &str| {
        let lines: Vec<&str> = text.lines().collect();
        let start = lines.iter().position(|l| l.starts_with("SECRETS_FILE=")).expect("SECRETS_FILE line");
        let setup = lines.iter().position(|l| *l == "setup_secrets() {").expect("setup_secrets");
        let end = setup + lines[setup..].iter().position(|l| *l == "}").expect("its end");
        lines[start..=end].join("\n")
    };
    // D-B3-SECRETS-COMMENT: the 2.x line names the Rails task; 3.0's names the command that replaces it (PLAN-A29).
    let expected = block(&old).replace(
        "            echo \"# here. Run 'rake deltabadger:encryption:derived_keys' to move off that.\"",
        "            echo \"# here. Run 'deltabadger encryption-derived-keys' to move off that.\"");
    assert_ne!(expected, block(&old), "the one changed line is still in docker-entrypoint.sh");
    assert_eq!(block(&new), expected);
}

#[test]
fn a_fresh_install_is_provisioned_with_all_three_values() {
    let dir = tempfile::tempdir().unwrap();
    let r = provision(dir.path(), &[]);
    assert_eq!(r["SECRET_KEY_BASE"].len(), 128);
    assert_eq!(r[PRIMARY].len(), 64);
    assert_eq!(r[SALT].len(), 64);
    assert_ne!(r[PRIMARY], r[SALT]);
}

#[test]
fn the_generated_file_is_not_world_readable() {
    let dir = tempfile::tempdir().unwrap();
    provision(dir.path(), &[]);
    assert_eq!(std::fs::metadata(dir.path().join(".secrets")).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn an_install_with_a_database_is_never_given_generated_encryption_keys() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("production.sqlite3"), "not empty").unwrap();
    let r = provision(dir.path(), &[("SECRET_KEY_BASE", "preserved-from-the-environment")]);
    assert_eq!(r[PRIMARY], "");
    assert_eq!(r[SALT], "");
    assert_eq!(r["SECRET_KEY_BASE"], "preserved-from-the-environment");
}

#[test]
fn an_empty_database_file_still_counts_as_a_database() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("production.sqlite3"), "").unwrap();
    assert_eq!(provision(dir.path(), &[])[PRIMARY], "");
}

#[test]
fn a_database_supplied_by_url_is_never_given_generated_encryption_keys() {
    for variable in ["DATABASE_URL", "PRIMARY_DATABASE_URL"] {
        let dir = tempfile::tempdir().unwrap();
        let r = provision(dir.path(), &[(variable, "sqlite3:/restored/production.sqlite3")]);
        assert_eq!(r[PRIMARY], "", "{variable} should have suppressed key generation");
        assert_eq!(r[SALT], "", "{variable} should have suppressed key generation");
    }
}

#[test]
fn an_existing_secrets_file_is_left_exactly_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".secrets");
    std::fs::write(&path, "SECRET_KEY_BASE=existing-value\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let r = provision(dir.path(), &[]);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "SECRET_KEY_BASE=existing-value\n");
    assert_eq!(r["SECRET_KEY_BASE"], "existing-value");
    assert_eq!(r[PRIMARY], "");
}

#[test]
fn one_key_supplied_externally_does_not_pick_up_the_other_from_the_file() {
    let dir = tempfile::tempdir().unwrap();
    provision(dir.path(), &[]);
    let r = provision(dir.path(), &[(PRIMARY, "supplied-externally")]);
    assert_eq!(r[PRIMARY], "supplied-externally");
    assert_eq!(r[SALT], "", "the salt must not be taken from the file");
}

#[test]
fn both_keys_supplied_externally_win_over_the_file() {
    let dir = tempfile::tempdir().unwrap();
    provision(dir.path(), &[]);
    let r = provision(dir.path(), &[(PRIMARY, "external-primary"), (SALT, "external-salt")]);
    assert_eq!(r[PRIMARY], "external-primary");
    assert_eq!(r[SALT], "external-salt");
}

#[test]
fn a_whitespace_only_supplied_key_does_not_suppress_the_stored_pair() {
    let dir = tempfile::tempdir().unwrap();
    let generated = provision(dir.path(), &[]);
    let r = provision(dir.path(), &[(PRIMARY, "   ")]);
    assert_eq!(r[PRIMARY], generated[PRIMARY]);
    assert_eq!(r[SALT], generated[SALT]);
}

#[test]
fn keys_supplied_externally_on_a_first_boot_are_not_shadowed_by_a_stored_pair() {
    let dir = tempfile::tempdir().unwrap();
    provision(dir.path(), &[(PRIMARY, "external-primary"), (SALT, "external-salt")]);
    assert!(!secrets_file(dir.path()).contains(&format!("{PRIMARY}=")));
    assert!(!secrets_file(dir.path()).contains(&format!("{SALT}=")));
}

#[test]
fn a_first_boot_with_external_keys_records_that_they_are_external() {
    let dir = tempfile::tempdir().unwrap();
    provision(dir.path(), &[(PRIMARY, "external-primary"), (SALT, "external-salt")]);
    assert!(secrets_file(dir.path()).contains(&format!("{MARKER}=true")));
    assert_eq!(provision(dir.path(), &[])[MARKER], "true");
}

#[test]
fn a_secret_supplied_externally_is_not_shadowed_by_a_stored_one() {
    let dir = tempfile::tempdir().unwrap();
    let r = provision(dir.path(), &[("SECRET_KEY_BASE", "supplied-externally")]);
    assert_eq!(r["SECRET_KEY_BASE"], "supplied-externally");
    assert!(!secrets_file(dir.path()).contains("SECRET_KEY_BASE="));
}

#[test]
fn losing_an_externally_supplied_secret_fails_closed_rather_than_inventing_one() {
    let dir = tempfile::tempdir().unwrap();
    provision(dir.path(), &[("SECRET_KEY_BASE", "supplied-externally")]);
    let out = run_setup(dir.path(), &[]);
    assert!(!out.status.success(), "it should refuse to continue without a secret");
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(text.contains("SECRET_KEY_BASE not found"), "{text}");
}

#[test]
fn the_file_is_read_back_when_it_already_holds_the_new_keys() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(provision(dir.path(), &[]), provision(dir.path(), &[]));
}

#[test]
fn concurrent_first_boots_all_adopt_the_same_keys() {
    let dir = tempfile::tempdir().unwrap();
    let out = bash(dir.path(), &[], &format!("for i in 1 2 3 4 5 6 7 8; do\n  (\n    setup_secrets >/dev/null 2>&1\n    \
        echo \"${{{PRIMARY}}}:${{{SALT}}}\"\n  ) &\ndone\nwait"));
    assert!(out.status.success(), "entrypoint failed: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let adopted: Vec<&str> = stdout.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    assert_eq!(adopted.len(), 8);
    assert!(adopted.iter().all(|a| *a == adopted[0]), "containers disagreed: {adopted:?}");
    // And what they adopted is what is on disk, not merely consistent with each other.
    let (primary, salt) = adopted[0].split_once(':').unwrap();
    assert!(secrets_file(dir.path()).contains(&format!("{PRIMARY}={primary}")));
    assert!(secrets_file(dir.path()).contains(&format!("{SALT}={salt}")));
    let leftovers = std::fs::read_dir(dir.path()).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with(".secrets")).count();
    assert_eq!(leftovers, 1, "a temporary file was left behind");
}

/// PLAN-A29: the encryption commands reach `deltabadger` with the instance's secrets loaded, as `check` does; `rake`
/// stays refused (64) and names them.
#[test]
fn the_encryption_commands_run_with_the_secrets_and_rake_names_them() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let stub = bin.join("deltabadger");
    std::fs::write(&stub, "#!/bin/bash\necho \"ran $* with ${SECRET_KEY_BASE:+a secret}\"\n").unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    for command in ["encryption-report", "encryption-reset --confirm clear-credentials", "encryption-derived-keys"] {
        let out = bash(dir.path(), &[("PATH", &path)], &format!("main {command}"));
        assert!(out.status.success(), "{command}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout).lines().last(), Some(format!("ran {command} with a secret").as_str()));
    }
    let out = bash(dir.path(), &[("PATH", &path)], "main rake deltabadger:encryption:reset");
    assert_eq!(out.status.code(), Some(64));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("This command isn't available in this version of Deltabadger."), "{err}");
    assert!(err.contains("use encryption-report, encryption-reset and encryption-derived-keys"), "{err}");
    let out = bash(dir.path(), &[("PATH", &path)], "main console");
    assert!(!String::from_utf8_lossy(&out.stderr).contains("encryption-report"), "only rake names them");
}
