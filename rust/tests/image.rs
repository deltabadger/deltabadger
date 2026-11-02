//! The 3.0 image (Dockerfile.rust), run as Docker runs it (PLAN-B3 §8). Every test needs `DELTABADGER_IMAGE` (an image
//! built from Dockerfile.rust) and a Docker daemon; without the variable they pass without doing anything, so the
//! ordinary test runs never need Docker. CI's `image` job builds the image and sets it.
#![cfg(unix)]

mod common;

use deltabadger::crypto::{Cipher, EncryptionKeys};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn image() -> Option<String> {
    let image = std::env::var("DELTABADGER_IMAGE").ok().filter(|v| !v.trim().is_empty());
    if image.is_none() { eprintln!("DELTABADGER_IMAGE is not set: skipped"); }
    image
}

fn docker(args: &[&str]) -> String {
    let out = docker_out(args);
    assert!(out.status.success(), "docker {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// `docker args`, whatever its exit code.
fn docker_out(args: &[&str]) -> Output { Command::new("docker").args(args).output().expect("docker runs") }

/// A name no other container or project of this run uses.
fn unique(prefix: &str) -> String {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    format!("{prefix}{}x{}", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst))
}

/// Runs `command`, which starts the container `name`, and gives it a minute to exit. One still running then is removed
/// and the test fails: a command that wrongly starts an engine fails instead of hanging.
fn exits(mut command: Command, name: &str) -> Output {
    let child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("docker runs");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || { let _ = tx.send(child.wait_with_output()); });
    match rx.recv_timeout(Duration::from_secs(60)) {
        Ok(out) => out.unwrap(),
        Err(_) => { let _ = docker_out(&["rm", "-f", name]); panic!("{name} was still running after a minute") }
    }
}

/// `docker run --rm` of `args` (options, image, command), bounded as `exits`.
fn run_once(args: &[&str]) -> Output {
    let name = unique("dbrun");
    let mut command = Command::new("docker");
    command.args(["run", "--rm", "--name", &name]).args(args);
    exits(command, &name)
}

fn id(flag: &str) -> String { String::from_utf8(Command::new("id").arg(flag).output().unwrap().stdout).unwrap().trim().to_string() }

/// What /app/storage is. A named volume, as the compose files use (`storage:/app/storage`): a Linux filesystem on every
/// host, so ownership reads as it does in production (Docker Desktop and OrbStack show bind-mounted host files as
/// root's). Or a temp dir bind-mounted there, for a container that runs as this user; dropping it gives its files back
/// to this user first, so the temp dir can be removed.
/// `At` is a mount (`<host dir>:/app/storage`) inside a folder another `Volume` owns and cleans up.
enum Volume { Named(String), Bind { dir: tempfile::TempDir, image: String }, At(String) }

impl Volume {
    fn named() -> Self { Volume::Named(docker(&["volume", "create"])) }
    fn bind(image: &str) -> Self { Volume::Bind { dir: tempfile::tempdir().unwrap(), image: image.into() } }
    fn mount(&self) -> String {
        match self { Volume::Named(name) => format!("{name}:/app/storage"), Volume::Bind { dir, .. } => format!("{}:/app/storage", dir.path().display()), Volume::At(mount) => mount.clone() }
    }
    fn path(&self) -> &Path { match self { Volume::Bind { dir, .. } => dir.path(), _ => panic!("only a Bind has a host path") } }
}

impl Drop for Volume {
    fn drop(&mut self) {
        match &*self {
            Volume::Named(name) => { let _ = Command::new("docker").args(["volume", "rm", "-f", name]).output(); }
            Volume::Bind { image, .. } => {
                let owner = format!("{}:{}", id("-u"), id("-g"));
                let _ = Command::new("docker").args(["run", "--rm", "--user", "0", "--entrypoint", "chown", "-v", &self.mount(), image, "-R", &owner, "/app/storage"]).output();
            }
            Volume::At(_) => {}
        }
    }
}

/// A running container, removed on drop.
struct Container { id: String, port: u16 }

impl Container {
    fn start(image: &str, volume: &Volume, extra: &[&str], command: &[&str]) -> Self {
        let mut args = vec!["run", "-d", "-p", "127.0.0.1::3000", "-e", "IBKR_CATALOGUE_URL=off"];
        let mount = volume.mount();
        args.extend(["-v", &mount]);
        args.extend(extra);
        args.push(image);
        args.extend(command);
        let id = docker(&args);
        let port = Self::mapped(&id);
        Container { id, port }
    }

    /// The host port Docker gave 3000 (a new one after every restart).
    fn mapped(id: &str) -> u16 {
        docker(&["port", id, "3000/tcp"]).lines().next().and_then(|l| l.rsplit(':').next()).and_then(|p| p.parse().ok()).expect("a mapped port")
    }

    fn restart(&mut self) {
        docker(&["restart", &self.id]);
        self.port = Self::mapped(&self.id);
    }

    fn exec(&self, command: &[&str]) -> String {
        let mut args = vec!["exec", &self.id];
        args.extend(command);
        docker(&args)
    }

    fn logs(&self) -> String {
        let out = Command::new("docker").args(["logs", &self.id]).output().unwrap();
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    }

    /// What the container wrote to stderr alone.
    fn stderr(&self) -> String {
        String::from_utf8_lossy(&Command::new("docker").args(["logs", &self.id]).output().unwrap().stderr).into_owned()
    }

    /// The HTTP status of GET `path`, or None while nothing answers.
    fn status(&self, path: &str) -> Option<u16> { self.get(path).map(|(status, _)| status) }

    /// GET `path`: its status and the whole response.
    fn get(&self, path: &str) -> Option<(u16, String)> {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port)).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        write!(stream, "GET {path} HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n").ok()?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response).ok()?;
        let response = String::from_utf8_lossy(&response).into_owned();
        Some((response.get(9..12)?.parse().ok()?, response))
    }

    /// `docker stop` (SIGTERM, Docker's 10 s): the exit code.
    fn stop(&self) -> String {
        docker(&["stop", &self.id]);
        docker(&["inspect", "-f", "{{.State.ExitCode}}", &self.id])
    }

    /// Waits up to two minutes for `path` to answer `want`.
    fn wait_for(&self, path: &str, want: u16) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(120) {
            if self.status(path) == Some(want) { return; }
            std::thread::sleep(Duration::from_millis(500));
        }
        panic!("{path} never answered {want}; last {:?}\n{}", self.status(path), self.logs());
    }
}

impl Drop for Container {
    fn drop(&mut self) { let _ = Command::new("docker").args(["rm", "-f", &self.id]).output(); }
}

/// The 2.x image's first start, without Ruby: as root on an empty volume it writes .secrets (mode 600, owned by 1000),
/// drops to 1000, and serve creates the install; a restart reads the same file and opens the same install.
#[test]
fn an_empty_volume_started_as_root_becomes_a_new_install_that_restarts_on_its_own_secrets() {
    let Some(image) = image() else { return };
    let volume = Volume::named();
    let mut app = Container::start(&image, &volume, &[], &[]);
    app.wait_for("/health-check", 200);
    assert_eq!(app.status("/ui-ready"), Some(200));
    assert_eq!(app.exec(&["stat", "-c", "%u %a", "/app/storage/.secrets"]), "1000 600");
    assert_eq!(app.exec(&["stat", "-c", "%u", "/app/storage/production.sqlite3"]), "1000");
    assert_eq!(app.exec(&["stat", "-c", "%u", "/proc/1"]), "1000", "the app (PID 1) runs as 1000, not root");
    let secrets = app.exec(&["cat", "/app/storage/.secrets"]);
    for key in ["SECRET_KEY_BASE=", "ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY=", "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT="] {
        assert!(secrets.contains(key), "{key} missing from\n{secrets}");
    }
    assert!(app.logs().contains("set up a new install"), "{}", app.logs());

    app.restart();
    app.wait_for("/health-check", 200);
    assert_eq!(app.exec(&["cat", "/app/storage/.secrets"]), secrets, "a restart never rewrites .secrets");
    assert!(!app.logs().contains("can't read this install"), "{}", app.logs());
}

/// `user: <uid>:<gid>` in a compose skips the ownership fix and the drop, as with the 2.x image.
#[test]
fn an_empty_volume_started_as_the_host_user_becomes_a_new_install() {
    let Some(image) = image() else { return };
    let volume = Volume::bind(&image);
    let user = format!("{}:{}", id("-u"), id("-g"));
    let app = Container::start(&image, &volume, &["--user", &user], &[]);
    app.wait_for("/health-check", 200);
    assert!(volume.path().join(".secrets").exists());
    assert!(volume.path().join("production.sqlite3").exists());
}

/// SIGTERM (`docker stop`) reaches serve as PID 1: it stops on request and exits 0, well inside Docker's 10 s.
#[test]
fn docker_stop_stops_serve_cleanly() {
    let Some(image) = image() else { return };
    let volume = Volume::named();
    let app = Container::start(&image, &volume, &[], &[]);
    app.wait_for("/health-check", 200);
    let started = Instant::now();
    docker(&["stop", &app.id]);
    assert!(started.elapsed() < Duration::from_secs(9), "took {:?}", started.elapsed());
    assert_eq!(docker(&["inspect", "-f", "{{.State.ExitCode}}", &app.id]), "0", "{}", app.logs());
    assert!(app.logs().contains("stopped on request"), "{}", app.logs());
}

// ---- B3-2: the first start on a 2.x volume (PLAN-B3 §2, §8 slice 2) ----

/// The last 2.x release. Its own entrypoint makes and reads every `.secrets` below, and its own `migrate` makes format 1.
const RAILS_2X: &str = "ghcr.io/deltabadger/deltabadger:2.70.21";
/// C9 (main.rs WRONG_SECRET), as the refusal page escapes it.
const C9: &str = "can&#39;t read this install with this SECRET_KEY_BASE";
/// A second pair and secret, for the starts that must be refused.
const OTHER_PAIR: [&str; 2] = ["ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY=b3-2-other-key", "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT=b3-2-other-salt"];
const OTHER_SECRET: &str = "SECRET_KEY_BASE=b3-2-other-secret";

fn uid_gid() -> String { format!("{}:{}", id("-u"), id("-g")) }

/// Runs `script` in the 2.70.21 image under bash with its docker-entrypoint.sh sourced (not run), on `dir` at /s with
/// SECRETS_FILE=/s/.secrets and the `-e` values in `env`, offline; the files go back to this user. Its stdout.
fn entrypoint_2x(dir: &Path, env: &[&str], script: &str) -> String {
    let mount = format!("{}:/s", dir.display());
    let script = format!("set -e; source /app/docker-entrypoint.sh; {script}; chown -R {} /s", uid_gid());
    let mut args = vec!["run", "--rm", "--network", "none", "--user", "0", "--entrypoint", "bash", "-v", &mount,
                        "-e", "DELTABADGER_ENTRYPOINT_SOURCED=1", "-e", "SECRETS_FILE=/s/.secrets"];
    for e in env { args.extend(["-e", e]); }
    args.extend([RAILS_2X, "-c", &script]);
    docker(&args)
}

/// The keys the install in `dir` is encrypted under, read the way 2.x reads them: 2.70.21's load_secrets over `dir`'s
/// `.secrets` with the container's `env` (env first), then EncryptionKeys::resolve over what it exported.
fn keys_2x(dir: &Path, env: &[&str]) -> (String, EncryptionKeys) {
    let out = entrypoint_2x(dir, env, "load_secrets >/dev/null; for v in SECRET_KEY_BASE ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY \
        ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT ACTIVE_RECORD_ENCRYPTION_KEYS_EXTERNAL; do printf '%s=%s\\n' \"$v\" \"${!v}\"; done");
    let vars: HashMap<String, String> = out.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.into(), v.into())).collect();
    let secret = vars["SECRET_KEY_BASE"].clone();
    let keys = EncryptionKeys::resolve(&|name| vars.get(name).cloned(), &secret).expect("2.x's values resolve");
    (secret, keys)
}

/// Copies `dir` into the named volume `volume` (root, ownership as in `dir`; the entrypoint gives it to 1000 on start).
fn put(image: &str, volume: &Volume, dir: &Path) {
    let src = format!("{}:/src", dir.display());
    docker(&["run", "--rm", "--user", "0", "--entrypoint", "cp", "-v", &volume.mount(), "-v", &src, image, "-a", "/src/.", "/app/storage/"]);
}

/// A copy of the volume on this host, owned by this user (for SQLite reads and the backup folder).
fn take(image: &str, volume: &Volume) -> tempfile::TempDir {
    let out = tempfile::tempdir().unwrap();
    let dst = format!("{}:/out", out.path().display());
    docker(&["run", "--rm", "--user", "0", "--entrypoint", "sh", "-v", &volume.mount(), "-v", &dst, image, "-c",
             &format!("cp -a /app/storage/. /out/ && chown -R {} /out", uid_gid())]);
    out
}

/// Every folder and every file's sha256 on the volume, `.secrets` included. As tests/cli.rs files_of: the engine lock,
/// SQLite's -shm and an empty -wal are incidental (taking the lock creates the first; opening a file the others).
fn snapshot(image: &str, volume: &Volume) -> String {
    docker(&["run", "--rm", "--user", "0", "--entrypoint", "sh", "-v", &volume.mount(), image, "-c",
             "cd /app/storage && find . -type d | sort && find . -type f ! -name .engine.lock ! -name '*-shm' ! \\( -name '*-wal' -empty \\) \
              -exec sha256sum {} + | sort -k2"])
}

/// A 2.x volume and what 3.0 needs to start on it: `env`, the container env this format runs with; `wrong`, an env
/// that must be refused with C9 (a different secret or pair from the one the data was written with); `cipher`, the
/// data's own keys.
struct TwoX { volume: Volume, env: Vec<String>, wrong: Vec<String>, cipher: Cipher }

/// Formats 2-5 (§2): the install exactly as Rails prepares it (rails_install_unmigrated), its `.secrets` written by
/// 2.70.21's generate_secrets under `gen_env` and then `edit`ed, copied into a named volume; then 2.70.21's
/// `rake runner` with the container env `env` stores an encrypted app_config, so Rails itself encrypts the one sample
/// under its own reading of that file and env. Not a Rust-encrypted seed: a Rust key derivation shared by the fixture
/// and the image would cancel out, and a sample Rust wrote would open under it. `cipher` (Rust's reading of the same
/// file) only reads the lease 3.0 writes.
fn two_x(image: &str, gen_env: &[&str], edit: impl FnOnce(&Path), env: &[&str], wrong: &[&str]) -> TwoX {
    let dir = common::rails_install_unmigrated();
    entrypoint_2x(dir.path(), gen_env, "generate_secrets");
    edit(dir.path());
    let (_, keys) = keys_2x(dir.path(), env);
    let volume = Volume::named();
    put(image, &volume, dir.path());
    rails_encrypts_a_sample(&volume, env);
    let cipher = Cipher::new(&keys);
    TwoX { volume, env: env.iter().map(|s| s.to_string()).collect(), wrong: wrong.iter().map(|s| s.to_string()).collect(), cipher }
}

fn e_args(env: &[String]) -> Vec<&str> { env.iter().flat_map(|e| ["-e", e.as_str()]).collect() }

/// A start this volume must refuse with C9: the refusal page on every path with 503, `/health-check` 503,
/// `/ui-ready` 200, SIGTERM → 0, and every file byte-equal. The non-vacuity half of each format: C9 refuses only when
/// the install holds an encrypted sample and none opens (PLAN-B2 r2.4), so a passing start on the same volume means
/// its keys read that sample.
fn refused_with_c9(image: &str, volume: &Volume, env: &[String]) {
    let before = snapshot(image, volume);
    let app = Container::start(image, volume, &e_args(env), &[]);
    app.wait_for("/health-check", 503);
    let (status, page) = app.get("/").unwrap();
    assert_eq!(status, 503);
    assert!(page.contains(C9), "{page}\n{}", app.logs());
    assert_eq!(app.status("/ui-ready"), Some(200));
    assert_eq!(app.stop(), "0", "{}", app.logs());
    drop(app);
    assert_eq!(snapshot(image, volume), before, "a refused start changes no file");
}

/// 2.70.21's `rake runner` (its entrypoint loads `.secrets` first, env first) stores an app_config Rails encrypts.
fn rails_encrypts_a_sample(volume: &Volume, env: &[&str]) {
    let mount = volume.mount();
    let mut args = vec!["run", "--rm", "--network", "none", "-v", &mount];
    for e in env { args.extend(["-e", e]); }
    args.extend([RAILS_2X, "rake", "runner", "print AppConfig.create!(key: 'coingecko_api_key', value: 'b3-2-sample').\
        class.connection.select_value(\"SELECT value FROM app_configs WHERE key = 'coingecko_api_key'\")"]);
    let stored = docker(&args);
    assert!(stored.lines().last().is_some_and(|l| l.starts_with("{\"p\":")), "Rails stored it encrypted: {stored}");
}

/// The takeover itself: `serve` claims the install (`/health-check` 200, the "took over" line, never C9), saves its
/// copy under backups/pre-3.0-<UTC>/ first, leaves `.secrets` byte-equal, and the lease reads `rust` under the data's
/// own keys. Stopped (0) after.
fn takes_over(image: &str, two_x: &TwoX) {
    let secrets = std::fs::read(take(image, &two_x.volume).path().join(".secrets")).unwrap();
    let app = Container::start(image, &two_x.volume, &e_args(&two_x.env), &[]);
    app.wait_for("/health-check", 200);
    let logs = app.logs();
    assert!(logs.contains("took over (") && !logs.contains("can't read this install"), "{logs}");
    assert_eq!(app.stop(), "0", "{}", app.logs());
    drop(app);
    left_taken_over(image, &two_x.volume, &two_x.cipher, &secrets);
}

/// The volume after a stopped takeover: `.secrets` equal to `secrets`, one backups/pre-3.0-<UTC>/, lease `rust`.
fn left_taken_over(image: &str, volume: &Volume, cipher: &Cipher, secrets: &[u8]) {
    let copy = take(image, volume);
    assert_eq!(std::fs::read(copy.path().join(".secrets")).unwrap(), secrets, "a takeover never rewrites .secrets");
    let backups: Vec<_> = std::fs::read_dir(copy.path().join("backups")).expect("a backups folder").flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert!(backups.len() == 1 && backups[0].starts_with("pre-3.0-"), "{backups:?}");
    let c = rusqlite::Connection::open(copy.path().join("production.sqlite3")).unwrap();
    let lease = deltabadger::lease::read(&c, cipher).unwrap().expect("a lease");
    assert_eq!(lease["engine"], "rust", "{lease}");
}

/// Format 1's volume (below): 2.70.21's `migrate` on an empty named volume, then a Rails-encrypted sample.
fn made_by_2x_migrate(image: &str) -> TwoX {
    let volume = Volume::named();
    docker(&["run", "--rm", "--network", "none", "-v", &volume.mount(), RAILS_2X, "migrate"]);
    rails_encrypts_a_sample(&volume, &[]);
    let (_, keys) = keys_2x(take(image, &volume).path(), &[]);
    TwoX { volume, env: vec![], wrong: OTHER_PAIR.map(String::from).to_vec(), cipher: Cipher::new(&keys) }
}

/// Format 1, real 2.x: 2.70.21's `migrate` on an empty volume creates the install and its `.secrets` (SECRET_KEY_BASE +
/// pair), and 2.70.21's `rake runner` stores an encrypted app_config, so Rails itself encrypted it under that file.
#[test]
fn format_1_a_volume_made_by_2x_migrate_is_taken_over() {
    let Some(image) = image() else { return };
    let fixture = made_by_2x_migrate(&image);
    refused_with_c9(&image, &fixture.volume, &fixture.wrong);
    takes_over(&image, &fixture);
}

/// Format 2, Umbrel's shape: SECRET_KEY_BASE in the env, the file with the pair only.
#[test]
fn format_2_secret_in_the_env_and_the_pair_in_the_file_is_taken_over() {
    let Some(image) = image() else { return };
    let env = ["SECRET_KEY_BASE=b3-2-umbrel-secret"];
    let fixture = two_x(&image, &env, |_| {}, &env, &[env[0], OTHER_PAIR[0], OTHER_PAIR[1]]);
    let file = std::fs::read_to_string(take(&image, &fixture.volume).path().join(".secrets")).unwrap();
    assert!(!file.contains("SECRET_KEY_BASE=") && file.contains("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY="), "{file}");
    refused_with_c9(&image, &fixture.volume, &fixture.wrong);
    takes_over(&image, &fixture);
}

/// Format 3, derived keys: the database was there before the file, so the file holds SECRET_KEY_BASE only and the keys
/// are derived from it. Also item 3 (wrong secret) on a volume 3.0 has taken over: C9, nothing changed.
#[test]
fn format_3_secret_only_with_derived_keys_is_taken_over() {
    let Some(image) = image() else { return };
    let fixture = two_x(&image, &["DATABASE_PATH=/s/production.sqlite3"], |_| {}, &[], &[OTHER_SECRET]);
    let file = std::fs::read_to_string(take(&image, &fixture.volume).path().join(".secrets")).unwrap();
    assert!(file.contains("SECRET_KEY_BASE=") && !file.contains("ACTIVE_RECORD_ENCRYPTION"), "{file}");
    refused_with_c9(&image, &fixture.volume, &fixture.wrong);
    takes_over(&image, &fixture);
    refused_with_c9(&image, &fixture.volume, &fixture.wrong);
}

/// Format 4, external keys: the pair in the env, the file with SECRET_KEY_BASE and the marker. Without the pair the
/// start is refused on the marker (EncryptionKeys' ExternalMarkerWithoutKeys) and nothing changes.
#[test]
fn format_4_external_keys_are_taken_over_and_refused_once_the_env_loses_them() {
    let Some(image) = image() else { return };
    let pair = ["ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY=b3-2-external-key", "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT=b3-2-external-salt"];
    let fixture = two_x(&image, &pair, |_| {}, &pair, &OTHER_PAIR);
    let file = std::fs::read_to_string(take(&image, &fixture.volume).path().join(".secrets")).unwrap();
    assert!(file.contains("ACTIVE_RECORD_ENCRYPTION_KEYS_EXTERNAL=true") && !file.contains("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY="), "{file}");
    refused_with_c9(&image, &fixture.volume, &fixture.wrong);

    let before = snapshot(&image, &fixture.volume);
    let app = Container::start(&image, &fixture.volume, &[], &[]);
    app.wait_for("/health-check", 503);
    assert!(app.stderr().contains("deltabadger: encryption keys: ExternalMarkerWithoutKeys"), "{}", app.logs());
    assert!(app.get("/").unwrap().1.contains("ExternalMarkerWithoutKeys"));
    assert_eq!(app.stop(), "0", "{}", app.logs());
    drop(app);
    assert_eq!(snapshot(&image, &fixture.volume), before, "the refused start changes no file, .secrets included");

    takes_over(&image, &fixture);
}

/// Format 5, an old file: SECRET_KEY_BASE with the legacy APP_ENCRYPTION_KEY / DEVISE_SECRET_KEY lines (written
/// before the pair existed, so the keys are derived) and CRLF line ends.
#[test]
fn format_5_an_old_crlf_file_with_legacy_keys_is_taken_over() {
    let Some(image) = image() else { return };
    let old = |dir: &Path| {
        let file = dir.join(".secrets");
        let new = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<String> = new.lines().filter(|l| !l.starts_with("ACTIVE_RECORD_ENCRYPTION_")).map(String::from)
            .chain(["APP_ENCRYPTION_KEY=0123456789abcdef0123456789abcdef".into(), "DEVISE_SECRET_KEY=fedcba9876543210fedcba9876543210".into()])
            .collect();
        std::fs::write(&file, lines.join("\r\n") + "\r\n").unwrap();
    };
    let fixture = two_x(&image, &[], old, &[], &[OTHER_SECRET]);
    let file = std::fs::read_to_string(take(&image, &fixture.volume).path().join(".secrets")).unwrap();
    assert!(file.contains("SECRET_KEY_BASE=") && file.contains("DEVISE_SECRET_KEY=") && file.contains("\r\n") && !file.contains("ACTIVE_RECORD"), "{file:?}");
    refused_with_c9(&image, &fixture.volume, &fixture.wrong);
    takes_over(&image, &fixture);
}

/// Behind (B2's C1): the newest Rails migrations missing. The refusal page with C1, `/health-check` 503, `/ui-ready`
/// 200, the operator's detail on stderr, SIGTERM → 0, and the volume exactly as it was: no backups folder, `.secrets`
/// and every data file byte-equal.
#[test]
fn a_behind_2x_volume_is_refused_untouched() {
    let Some(image) = image() else { return };
    let behind = |dir: &Path| {
        rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap().execute(
            "DELETE FROM schema_migrations WHERE version IN (SELECT version FROM schema_migrations ORDER BY version DESC LIMIT 3)", []).unwrap();
    };
    let TwoX { volume, .. } = two_x(&image, &["DATABASE_PATH=/s/production.sqlite3"], behind, &[], &[]);
    let before = snapshot(&image, &volume);

    let app = Container::start(&image, &volume, &[], &[]);
    app.wait_for("/health-check", 503);
    let (status, page) = app.get("/").unwrap();
    assert_eq!(status, 503);
    assert!(page.contains("this data is from an older version of Deltabadger"), "{page}");
    assert_eq!(app.status("/ui-ready"), Some(200));
    assert!(app.stderr().contains("this data is behind this build (3 migration(s) missing)"), "{}", app.logs());
    assert_eq!(app.stop(), "0", "{}", app.logs());
    drop(app);
    let after = snapshot(&image, &volume);
    assert_eq!(after, before, "a Behind volume is not touched");
    assert!(!after.contains("./backups"), "{after}");
}

/// Launchpad's shape (hosted): no command, started as root, its env. SECRET_KEY_BASE from the env; the rest is
/// ignored or read as before (PLAN-B3 §1.2) and does not stop the takeover.
#[test]
fn launchpads_shape_takes_a_2x_volume_over() {
    let Some(image) = image() else { return };
    let env = ["SECRET_KEY_BASE=b3-2-hosted-secret", "AUTO_MIGRATE=true", "SOLID_QUEUE_IN_PUMA=true", "APP_ROOT_URL=https://b3-2.example.test",
               "MCP_ENABLED=true", "DELTABADGER_UPDATE_CHECK=false", "SETUP_TOKEN=b3-2-setup-token"];
    let fixture = two_x(&image, &env[..1], |_| {}, &env, &[]);
    takes_over(&image, &fixture);
}

// ---- B3-3: the commands and both compose files (PLAN-B3 §1.3, §3, §8 slice 3) ----

/// J1, M1, R1 (docker/entrypoint.sh, PLAN-B3 §3 as approved).
const J1: &str = "This service no longer does anything: Deltabadger now runs its background work in its main service. You can remove this one.";
const M1: &str = "Nothing to do: Deltabadger updates its data itself when it starts.";
const R1: &str = "This command isn't available in this version of Deltabadger.";
/// main.rs on LeaseError::Locked.
const LOCKED: &str = "deltabadger: another Deltabadger engine is running on this data";

/// A compose project: the repository's compose `file` as it is, plus an override written here, under a project name no
/// other test uses, with `env` as the compose's own environment. `down -v` on drop, after a failure too.
struct Project { name: String, file: String, overrides: tempfile::TempDir, env: Vec<(String, String)> }

impl Project {
    fn new(file: &str, override_yaml: &str, env: &[(&str, &str)]) -> Self {
        let name = unique("dbimage");
        let overrides = tempfile::tempdir().unwrap();
        std::fs::write(overrides.path().join("override.yml"), override_yaml).unwrap();
        let file = format!("{}/../{file}", env!("CARGO_MANIFEST_DIR"));
        Project { name, file, overrides, env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
    }

    fn command(&self, args: &[&str]) -> Command {
        let over = self.overrides.path().join("override.yml");
        let mut command = Command::new("docker");
        command.args(["compose", "-p", &self.name, "-f", &self.file, "-f", over.to_str().unwrap()]).args(args)
            .envs(self.env.iter().map(|(k, v)| (k, v)));
        command
    }

    /// `docker compose <args>` on this project, whatever its exit code.
    fn compose(&self, args: &[&str]) -> Output { self.command(args).output().expect("docker compose runs") }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.compose(args);
        assert!(out.status.success(), "docker compose {args:?}: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// The service's container, reached on its published port 3000 (when it has one).
    fn container(&self, service: &str) -> Container {
        let id = self.ok(&["ps", "-q", service]);
        assert!(!id.is_empty(), "{service} has no container: {}", self.logs());
        let port = docker_out(&["port", &id, "3000/tcp"]).stdout;
        let port = String::from_utf8_lossy(&port).lines().next().and_then(|l| l.rsplit(':').next()).and_then(|p| p.parse().ok()).unwrap_or(0);
        Container { id, port }
    }

    fn logs(&self) -> String {
        let out = self.compose(&["logs", "--no-color"]);
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
    }
}

impl Drop for Project {
    fn drop(&mut self) { let _ = self.compose(&["down", "-v", "--remove-orphans", "-t", "5"]); }
}

/// Every path on the volume (without the lock and SQLite's -wal/-shm, which a running engine changes) and `.secrets`'
/// sha256: what a refused start must leave as it was while the first engine keeps writing its databases.
fn paths(image: &str, volume: &Volume) -> String {
    docker(&["run", "--rm", "--user", "0", "--entrypoint", "sh", "-v", &volume.mount(), image, "-c",
             "cd /app/storage && find . ! -name .engine.lock ! -name '*-shm' ! -name '*-wal' | sort && sha256sum .secrets"])
}

/// Umbrel's compose (deltabadger/docker-compose.yml) as 2.x users have it, on a 2.x Umbrel install (format 2: the
/// secret is `${APP_SEED}-secret-key-base` in the env, the pair in `.secrets`). The override only swaps both images for
/// the 3.0 one, drops `app_proxy` (Umbrel's own) and publishes `web`'s 3000 for the test. One engine trades: `web`
/// takes the install over; `jobs` waits for `web` to be healthy, logs J1 and runs no `deltabadger`; a second engine
/// started through the compose (`run --rm web serve`, `run --rm web run`) is refused as Locked and writes nothing.
/// `jobs`' own health is not checked: its Solid Queue probe reports unhealthy on 3.0 (PLAN-B3 §1.4, cosmetic; B5
/// deletes the service).
#[test]
fn umbrels_two_service_compose_runs_one_engine() {
    let Some(image) = image() else { return };
    let seed = "b3-3-umbrel-seed";
    let secret = format!("SECRET_KEY_BASE={seed}-secret-key-base");
    let fixture = two_x(&image, &[&secret], |_| {}, &[&secret], &[]);
    // APP_DATA_DIR, with the install in its data/storage. A bind, as Umbrel mounts it; its files go back to this user
    // on drop (the entrypoint gives them to 1000).
    let data = Volume::bind(&image);
    std::fs::create_dir_all(data.path().join("data/storage")).unwrap();
    std::fs::create_dir_all(data.path().join("data/logs")).unwrap();
    let storage = Volume::At(format!("{}/data/storage:/app/storage", data.path().display()));
    docker(&["run", "--rm", "--user", "0", "--entrypoint", "cp", "-v", &fixture.volume.mount(), "-v", &format!("{}/data/storage:/out", data.path().display()),
             &image, "-a", "/app/storage/.", "/out/"]);
    let secrets = std::fs::read(take(&image, &storage).path().join(".secrets")).unwrap();

    let override_yaml = format!("services:\n  app_proxy: !reset null\n  web:\n    image: {image}\n    ports: [\"127.0.0.1::3000\"]\n  jobs:\n    image: {image}\n");
    let app_data_dir = data.path().display().to_string();
    let project = Project::new("deltabadger/docker-compose.yml", &override_yaml,
                               &[("APP_DATA_DIR", &app_data_dir), ("APP_SEED", seed), ("DEVICE_DOMAIN_NAME", "umbrel.local")]);

    project.ok(&["up", "-d", "web"]);
    let web = project.container("web");
    web.wait_for("/health-check", 200);
    assert_eq!(std::fs::read(take(&image, &storage).path().join(".secrets")).unwrap(), secrets, "web's takeover keeps .secrets");

    // The whole compose, as Umbrel starts it: `jobs` only once `web` is healthy (the image HEALTHCHECK).
    project.ok(&["up", "-d"]);
    let jobs = project.container("jobs");
    let start = Instant::now();
    while !jobs.logs().contains(J1) && start.elapsed() < Duration::from_secs(30) { std::thread::sleep(Duration::from_millis(500)); }
    assert!(jobs.logs().contains(J1), "{}", project.logs());
    assert_eq!(docker(&["inspect", "-f", "{{.State.Running}} {{.RestartCount}}", &jobs.id]), "true 0", "{}", project.logs());
    let top = docker(&["top", &jobs.id, "-o", "pid,comm"]);
    assert!(top.lines().any(|l| l.ends_with(" sleep")), "the idle loop shows:\n{top}");
    assert!(!top.lines().any(|l| l.split_whitespace().nth(1) == Some("deltabadger")), "jobs runs no engine:\n{top}");
    assert_eq!(std::fs::read(take(&image, &storage).path().join(".secrets")).unwrap(), secrets, "jobs keeps .secrets");
    let logs = project.logs();
    assert_eq!(logs.matches("took over (").count(), 1, "one takeover across both services:\n{logs}");

    // A second engine on the same data, through the same compose: refused, nothing written, the first one still serves.
    for command in ["serve", "run"] {
        let before = paths(&image, &storage);
        let name = unique("dbsecond");
        let out = exits(project.command(&["run", "--rm", "-T", "--no-deps", "--name", &name, "web", command]), &name);
        let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert_eq!(out.status.code(), Some(1), "{command}: {said}");
        assert!(said.contains(LOCKED), "{command}: {said}");
        assert_eq!(paths(&image, &storage), before, "a refused {command} writes nothing");
        assert_eq!(web.status("/health-check"), Some(200));
    }

    // `jobs` stops on SIGTERM at once with 0; `web` keeps serving.
    let start = Instant::now();
    assert_eq!(jobs.stop(), "0", "{}", jobs.logs());
    assert!(start.elapsed() < Duration::from_secs(2), "jobs took {:?} to stop", start.elapsed());
    assert_eq!(web.status("/health-check"), Some(200));

    assert_eq!(web.stop(), "0", "{}", web.logs());
    left_taken_over(&image, &storage, &fixture.cipher, &secrets);
}

/// The own-server compose (docker-compose.yml, `standalone`) on a real 2.x volume (format 1). The override swaps the
/// image and points `storage` at this test's volume; APP_PORT is a free port.
#[test]
fn the_own_server_compose_takes_a_2x_volume_over() {
    let Some(image) = image() else { return };
    let fixture = made_by_2x_migrate(&image);
    let secrets = std::fs::read(take(&image, &fixture.volume).path().join(".secrets")).unwrap();
    let Volume::Named(name) = &fixture.volume else { unreachable!() };
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port().to_string();
    let project = Project::new("docker-compose.yml",
        &format!("services:\n  deltabadger:\n    image: {image}\nvolumes:\n  storage:\n    name: {name}\n    external: true\n"),
        &[("APP_PORT", &port)]);
    project.ok(&["up", "-d"]);
    let app = project.container("deltabadger");
    app.wait_for("/health-check", 200);
    assert!(app.logs().contains("took over ("), "{}", app.logs());
    project.ok(&["stop"]);
    assert_eq!(docker(&["inspect", "-f", "{{.State.ExitCode}}", &app.id]), "0", "{}", app.logs());
    left_taken_over(&image, &fixture.volume, &fixture.cipher, &secrets);
}

/// `migrate` and `setup` on an empty volume, started as root: exit 0, M1, a `.secrets` (600, owned by 1000) and nothing
/// else: no database, no backups. `serve` on the same volume then creates the install on that same `.secrets`.
#[test]
fn migrate_and_setup_on_an_empty_volume_write_secrets_only() {
    let Some(image) = image() else { return };
    for command in ["migrate", "setup"] {
        let volume = Volume::named();
        let out = run_once(&["-v", &volume.mount(), &image, command]);
        assert_eq!(out.status.code(), Some(0), "{command}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stdout).contains(M1), "{command}: {}", String::from_utf8_lossy(&out.stdout));
        let state = docker(&["run", "--rm", "--user", "0", "--entrypoint", "sh", "-v", &volume.mount(), &image, "-c",
                             "cd /app/storage && find . | sort && stat -c '%u %a' .secrets"]);
        assert_eq!(state, ".\n./.secrets\n1000 600", "{command} leaves .secrets only");
        let secrets = std::fs::read(take(&image, &volume).path().join(".secrets")).unwrap();

        let app = Container::start(&image, &volume, &[], &[]);
        app.wait_for("/health-check", 200);
        assert!(app.logs().contains("set up a new install"), "{}", app.logs());
        assert_eq!(app.stop(), "0", "{}", app.logs());
        drop(app);
        assert_eq!(std::fs::read(take(&image, &volume).path().join(".secrets")).unwrap(), secrets, "serve keeps {command}'s .secrets");
    }
}

/// `console`, `rake …` and `export` (Rails' own) on a 2.x volume: exit 64, R1 on stderr, every file byte-equal.
#[test]
fn rails_commands_are_refused_and_change_nothing() {
    let Some(image) = image() else { return };
    let fixture = made_by_2x_migrate(&image);
    let before = snapshot(&image, &fixture.volume);
    for command in [&["console"][..], &["rake", "db:migrate"], &["export"]] {
        let mount = fixture.volume.mount();
        let mut args = vec!["--network", "none", "-v", &mount, &image];
        args.extend(command);
        let out = run_once(&args);
        assert_eq!(out.status.code(), Some(64), "{command:?}: {}", String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stderr).contains(R1), "{command:?}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(snapshot(&image, &fixture.volume), before, "{command:?} changes no file");
    }
}
