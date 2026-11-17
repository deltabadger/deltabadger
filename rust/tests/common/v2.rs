//! The published 2.x image, for the interop tests where 2.x acts on what Rust wrote (PLAN-B11 T8, r1.1): they run
//! only where the image job runs, with `DELTABADGER_IMAGE` set (image.rs's convention); elsewhere they skip.
use std::path::Path;
use std::process::{Command, Output};

/// The last 2.x release, as users run it.
pub const IMAGE: &str = "ghcr.io/deltabadger/deltabadger:2.70.21";

/// Whether this run has Docker and the image job's variable; says "skipped" when not.
pub fn enabled() -> bool {
    let on = std::env::var("DELTABADGER_IMAGE").ok().is_some_and(|v| !v.trim().is_empty());
    if !on { eprintln!("DELTABADGER_IMAGE is not set: skipped (a 2.70.21 interop test)"); }
    on
}

fn id(flag: &str) -> String { String::from_utf8(Command::new("id").arg(flag).output().unwrap().stdout).unwrap().trim().to_string() }

/// `script` under bash in the 2.70.21 image, in /app, offline, as root, with each `(host dir, container dir)` of
/// `mounts` bind-mounted and the `env` values set; every mount is given back to this user after (CI's bind mounts
/// keep the host's ownership). Its output, whatever the exit code.
pub fn run(mounts: &[(&Path, &str)], env: &[(&str, &str)], script: &str) -> Output {
    let owned: Vec<&str> = mounts.iter().map(|(_, at)| *at).collect();
    let script = format!("cd /app; ( {script} ); s=$?; chown -R {}:{} {}; exit $s", id("-u"), id("-g"), owned.join(" "));
    let mut command = Command::new("docker");
    command.args(["run", "--rm", "--network", "none", "--user", "0", "--entrypoint", "bash"]);
    for (host, at) in mounts { command.arg("-v").arg(format!("{}:{at}", host.display())); }
    for (k, v) in env { command.arg("-e").arg(format!("{k}={v}")); }
    command.args([IMAGE, "-c", &script]).output().expect("docker runs")
}

/// 2.70.21 (production env, as it ships) on the install in `dir` (its primary and queue), with cache and cable
/// in `scratch` and the encryption keys derived from `secret`: `rails <args>`, each arg quoted. `dir` is at /s and
/// `scratch` at /x inside, so a script file in `scratch` is `/x/<name>`.
pub fn v2(dir: &Path, scratch: &Path, secret: &str, args: &[&str]) -> Output { v2_in("production", dir, scratch, secret, args) }

/// `rails`, in the 2.x environment `env` (the recording harness's `test`, as it ran on the 2.x repo).
pub fn v2_in(env: &str, dir: &Path, scratch: &Path, secret: &str, args: &[&str]) -> Output {
    let keys = deltabadger::crypto::EncryptionKeys::resolve(&|_| None, secret).unwrap();
    let quoted: Vec<String> = args.iter().map(|a| format!("'{}'", a.replace('\'', r"'\''"))).collect();
    run(&[(dir, "/s"), (scratch, "/x")],
        &[("RAILS_ENV", env), ("SKIP_TEST_DATABASE", "true"), ("APP_ROOT_URL", "http://localhost:3000"), ("RAILS_LOG_TO_STDOUT", ""),
          ("SECRET_KEY_BASE", secret), ("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY", &keys.primary_key),
          ("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT", &keys.key_derivation_salt),
          ("DATABASE_PATH", "/s/production.sqlite3"), ("QUEUE_DATABASE_PATH", "/s/production_queue.sqlite3"),
          ("CACHE_DATABASE_PATH", "/x/cache.sqlite3"), ("CABLE_DATABASE_PATH", "/x/cable.sqlite3"),
          ("PRIMARY_DATABASE_URL", "sqlite3:/s/production.sqlite3"), ("QUEUE_DATABASE_URL", "sqlite3:/s/production_queue.sqlite3"),
          ("CACHE_DATABASE_URL", "sqlite3:/x/cache.sqlite3"), ("CABLE_DATABASE_URL", "sqlite3:/x/cable.sqlite3"), ("SCHEMA", "/x/schema.rb")],
        &format!("bundle exec rails {}", quoted.join(" ")))
}

/// What 2.70.21 decides for each recorded Continue case, run on a Rust-built install
/// (`baseline_install`) by fixtures/v2/continue_start.rb: the action_race::continue_* tests' 2.x side.
pub fn continue_start() -> Result<serde_json::Value, String> {
    let dir = super::baseline_install();
    let scratch = tempfile::tempdir().unwrap();
    std::fs::write(scratch.path().join("continue.rb"), include_str!("../fixtures/v2/continue_start.rb")).unwrap();
    let out = v2(dir.path(), scratch.path(), "engine-test-secret", &["runner", "/x/continue.rb"]);
    if !out.status.success() { return Err(format!("2.70.21's Continue recorder: {}", String::from_utf8_lossy(&out.stderr))); }
    serde_json::from_str(&std::fs::read_to_string(scratch.path().join("continue.json")).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}
