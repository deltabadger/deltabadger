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

/// Runs `command`, which starts the container `name`, and gives it a minute to exit. Past that the test fails: a
/// command that wrongly starts an engine fails instead of hanging. The client is killed and reaped first, so it can't
/// create the container after the cleanup; then the container is removed until Docker no longer lists it.
fn exits(mut command: Command, name: &str) -> Output {
    let child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("docker runs");
    let pid = child.id().to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || { let _ = tx.send(child.wait_with_output()); });
    if let Ok(out) = rx.recv_timeout(Duration::from_secs(60)) { return out.unwrap(); }
    let _ = Command::new("kill").args(["-9", &pid]).status();
    let _ = rx.recv();
    let filter = format!("name=^{name}$");
    let start = Instant::now();
    loop {
        let _ = docker_out(&["rm", "-f", name]);
        if docker(&["ps", "-a", "-q", "--filter", &filter]).is_empty() { break; }
        assert!(start.elapsed() < Duration::from_secs(30), "{name} could not be removed");
        std::thread::sleep(Duration::from_millis(500));
    }
    panic!("{name} was still running after a minute")
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
    fn start(image: &str, volume: &Volume, extra: &[&str], command: &[&str]) -> Self { Self::run(image, volume, true, extra, command) }

    /// As `start`, on `--network none`: nothing reachable outside the container, so no venue and no order; nothing is
    /// published either (port 0), and `status` asks from inside with the image's curl.
    fn offline(image: &str, volume: &Volume, extra: &[&str], command: &[&str]) -> Self { Self::run(image, volume, false, extra, command) }

    fn run(image: &str, volume: &Volume, publish: bool, extra: &[&str], command: &[&str]) -> Self {
        let mut args = vec!["run", "-d", "-e", "IBKR_CATALOGUE_URL=off"];
        args.extend(if publish { ["-p", "127.0.0.1::3000"] } else { ["--network", "none"] });
        let mount = volume.mount();
        args.extend(["-v", &mount]);
        args.extend(extra);
        args.push(image);
        args.extend(command);
        let id = docker(&args);
        let port = if publish { Self::mapped(&id) } else { 0 };
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
    /// Without a published port (`offline`, a compose service on `network_mode: none`) it is asked from inside.
    fn status(&self, path: &str) -> Option<u16> {
        if self.port != 0 { return self.get(path).map(|(status, _)| status) }
        let url = format!("http://localhost:3000{path}");
        // 5 s, as `get`'s read timeout: a server that accepts and never answers can't outlast `wait_for`'s deadline.
        let out = docker_out(&["exec", &self.id, "curl", "-s", "--max-time", "5", "-o", "/dev/null", "-w", "%{http_code}", &url]);
        String::from_utf8_lossy(&out.stdout).trim().parse().ok().filter(|&status| status != 0) // 000: nothing answers
    }

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

    /// `stop` that must exit 0 within `limit`: how long it took.
    fn stops_within(&self, limit: Duration) -> Duration {
        let start = Instant::now();
        assert_eq!(self.stop(), "0", "{}", self.logs());
        let took = start.elapsed();
        assert!(took < limit, "took {took:?} to stop\n{}", self.logs());
        took
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

/// r4.1's operator line for Behind (main.rs behind_detail), approved in r5 Q5.
fn behind_detail(missing: usize) -> String {
    format!("this data is behind this build ({missing} migration(s) missing): run Deltabadger 2.70.21's `migrate` on it once (or start 2.70.21 until it has finished starting), then start this again")
}

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
    docker(&["run", "--rm", "--network", "none", "--user", "0", "--entrypoint", "cp", "-v", &volume.mount(), "-v", &src, image, "-a", "/src/.", "/app/storage/"]);
}

/// A copy of the volume on this host, owned by this user (for SQLite reads and the backup folder).
fn take(image: &str, volume: &Volume) -> tempfile::TempDir {
    let out = tempfile::tempdir().unwrap();
    let dst = format!("{}:/out", out.path().display());
    docker(&["run", "--rm", "--network", "none", "--user", "0", "--entrypoint", "sh", "-v", &volume.mount(), "-v", &dst, image, "-c",
             &format!("cp -a /app/storage/. /out/ && chown -R {} /out", uid_gid())]);
    out
}

/// Every folder and every file's sha256 on the volume, `.secrets` included. As tests/cli.rs files_of: the engine lock,
/// SQLite's -shm and an empty -wal are incidental (taking the lock creates the first; opening a file the others).
fn snapshot(image: &str, volume: &Volume) -> String {
    docker(&["run", "--rm", "--network", "none", "--user", "0", "--entrypoint", "sh", "-v", &volume.mount(), image, "-c",
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
    app.stops_within(Duration::from_secs(2)); // the refusal page stops at once (r3.1)
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
    let owned = lease(image, volume, cipher).expect("a lease");
    assert_eq!(owned["engine"], "rust", "{owned}");
}

/// The lease row (app_configs engine_lease) on the volume, read under `cipher`; None once there is no row.
fn lease(image: &str, volume: &Volume, cipher: &Cipher) -> Option<serde_json::Value> {
    let copy = take(image, volume);
    deltabadger::lease::read(&rusqlite::Connection::open(copy.path().join("production.sqlite3")).unwrap(), cipher).unwrap()
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

/// C1 (main.rs BEHIND): the user's line on a Behind install, naming Umbrel's way too (PLAN-B5 r6 Q9' A).
const C1: &str = "Deltabadger didn't start: this data is from an older version of Deltabadger. Go back to your previous Deltabadger, \
                  update it to its latest version and start it. Once it has finished starting, stop it and start this again. On Umbrel, \
                  update Deltabadger in the Umbrel App Store instead. Nothing was changed.";
/// C1-U (main.rs BEHIND_UMBREL): C1 where DELTABADGER_PLATFORM is umbrel (PLAN-B5 r2.2).
const C1_UMBREL: &str = "Deltabadger didn't start: this data is from an older version of Deltabadger. Update Deltabadger in the Umbrel \
                         App Store: the update brings your data up to date first. If the update button isn't available, restart your \
                         Umbrel first. Nothing was changed.";

/// A line as the refusal page writes it (HTML-escaped apostrophes).
fn on_page(line: &str) -> String { line.replace('\'', "&#39;") }

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
    assert!(page.contains(&on_page(C1)), "{page}");
    assert_eq!(app.status("/ui-ready"), Some(200));
    assert!(app.stderr().contains(&format!("deltabadger: {C1}\ndeltabadger: {}", behind_detail(3))), "{}", app.logs());
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
/// The Umbrel compose of 2.60.0 and 2.70.21 (deltabadger/docker-compose.yml at those tags), frozen: an installed app
/// keeps the compose of its last Update (PLAN-B5 r5.1), so a 2.x install runs under its own version's file, never under
/// the one the repository ships. 2.34.0's (no DELTABADGER_PLATFORM, `jobs` waits for no health) is frozen beside it.
const UMBREL_2X: &str = "rust/tests/fixtures/umbrel/2.70.21-docker-compose.yml";

/// The frozen composes are the released files, byte for byte (`git show v<version>:deltabadger/docker-compose.yml`;
/// 2.70.21-1, B5-0's, was released by merge 70368fb0a, untagged). Needs no Docker.
#[test]
fn the_frozen_umbrel_composes_are_the_tagged_files() {
    use sha2::{Digest, Sha256};
    for (version, sha256) in [("2.34.0", "be91d372d29aa2e75244377aeb691b0796980fd87a76b14c9976f64a0c886caa"),
                              ("2.70.21", "2c91f1245d1cbd20198b40d753702d9c53ee1383ad1ee0731520fb4a9097e09e"),
                              ("2.70.21-1", "e56f28b18128c245350de6276bc74f336bffbb06f7260b1a059c9b1e8036ef54")] {
        let file = format!("{}/tests/fixtures/umbrel/{version}-docker-compose.yml", env!("CARGO_MANIFEST_DIR"));
        assert_eq!(hex::encode(Sha256::digest(std::fs::read(&file).unwrap())), sha256, "{file} is v{version}'s deltabadger/docker-compose.yml");
    }
}

const J1: &str = "This service no longer does anything: Deltabadger now runs its background work in its main service. You can remove this one.";
const M1: &str = "Nothing to do: Deltabadger updates its data itself when it starts.";
const R1: &str = "This command isn't available in this version of Deltabadger.";
/// main.rs on LeaseError::Locked.
const LOCKED: &str = "deltabadger: another Deltabadger engine is running on this data";

/// A compose project: the repository's compose `file` as it is, plus an override written here, under a project name no
/// other test uses, with `env` as the compose's own environment. `down -v` on drop, after a failure too.
struct Project { name: String, files: Vec<String>, _overrides: tempfile::TempDir, env: Vec<(String, String)> }

impl Project {
    fn new(file: &str, override_yaml: &str, env: &[(&str, &str)]) -> Self {
        Self::layered(&format!("{}/../{file}", env!("CARGO_MANIFEST_DIR")), &[override_yaml], env)
    }

    /// The compose file at `file`, then each of `overrides` in order (a later one wins).
    fn layered(file: &str, overrides: &[&str], env: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut files = vec![file.to_string()];
        for (i, yaml) in overrides.iter().enumerate() {
            let path = dir.path().join(format!("override{i}.yml"));
            std::fs::write(&path, yaml).unwrap();
            files.push(path.display().to_string());
        }
        Project { name: unique("dbimage"), files, _overrides: dir, env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("docker");
        command.args(["compose", "-p", &self.name]);
        for file in &self.files { command.args(["-f", file]); }
        command.args(args).envs(self.env.iter().map(|(k, v)| (k, v)));
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
    // `--rmi local`: the image a `build:` service made (named after the project) goes too.
    fn drop(&mut self) { let _ = self.compose(&["down", "-v", "--rmi", "local", "--remove-orphans", "-t", "5"]); }
}

/// Umbrel's compose as 2.x users have it (UMBREL_2X, frozen), on a 2.x Umbrel install (format 2: the
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
    let project = Project::new(UMBREL_2X, &override_yaml,
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
    // `web` is paused around each (its lock stays held), so every file's bytes can be compared, databases included.
    for command in ["serve", "run"] {
        docker(&["pause", &web.id]);
        let before = snapshot(&image, &storage);
        let name = unique("dbsecond");
        let out = exits(project.command(&["run", "--rm", "-T", "--no-deps", "--name", &name, "web", command]), &name);
        let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert_eq!(out.status.code(), Some(1), "{command}: {said}");
        assert!(said.contains(LOCKED), "{command}: {said}");
        assert_eq!(snapshot(&image, &storage), before, "a refused {command} writes nothing");
        docker(&["unpause", &web.id]);
        assert_eq!(web.status("/health-check"), Some(200));
    }

    // `jobs` stops on SIGTERM at once with 0; `web` keeps serving.
    jobs.stops_within(Duration::from_secs(2));
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
    assert_eq!(docker(&["inspect", "-f", "{{.Config.StopTimeout}}", &app.id]), "60", "stop_grace_period (Q2)");
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

// ---- B3-4: stop and handback (PLAN-B3 §1.4, §3, §8 slice 4 as r3.1 rewrote it) ----

/// C8 (main.rs hand_back), handback's success line.
const C8: &str = "scheduled. You can start Deltabadger 2.70.21 now.";

/// The build-from-source compose's config: it builds Dockerfile.rust (B3-7) and gives its service 60 s to stop (Q2).
/// Read before anything builds, so a revert to `build: .` (the Rails Dockerfile) fails here first.
#[test]
fn the_build_compose_builds_dockerfile_rust_and_gives_its_service_60_s_to_stop() {
    let Some(_) = image() else { return };
    let file = format!("{}/../{BUILD_COMPOSE}", env!("CARGO_MANIFEST_DIR"));
    let config = docker(&["compose", "-f", &file, "config"]);
    assert!(config.contains("dockerfile: Dockerfile.rust"), "{config}");
    assert!(config.contains("stop_grace_period: 1m0s"), "{config}");
}

/// A 2.x volume (format 1) 3.0 has taken over, served offline. SIGKILL (what Docker sends once the stop timeout runs
/// out) leaves it taken over; `docker start` continues the same ownership (AfterCrash: no second takeover, no second
/// backup); then a default `docker stop` (SIGTERM, 10 s) stops it on request with 0, well inside the 10 s. That an
/// order survives the kill is the engine's, proven in-process (tests/placement.rs, tests/signal.rs,
/// tests/withdrawal_engine.rs), not here (r3.1): offline, nothing is placed.
#[test]
fn a_taken_over_volume_restarts_after_a_kill_and_stops_on_sigterm() {
    let Some(image) = image() else { return };
    let fixture = made_by_2x_migrate(&image);
    let secrets = std::fs::read(take(&image, &fixture.volume).path().join(".secrets")).unwrap();
    let app = Container::offline(&image, &fixture.volume, &[], &[]);
    app.wait_for("/health-check", 200);
    docker(&["kill", "-s", "KILL", &app.id]);
    assert_eq!(docker(&["inspect", "-f", "{{.State.ExitCode}}", &app.id]), "137");
    left_taken_over(&image, &fixture.volume, &fixture.cipher, &secrets);

    docker(&["start", &app.id]);
    app.wait_for("/health-check", 200);
    let logs = app.logs();
    assert_eq!(logs.matches("took over (").count(), 2, "{logs}");
    assert!(logs.contains("took over (FromRails)") && logs.contains("took over (AfterCrash)"), "{logs}");
    let took = app.stops_within(Duration::from_secs(9));
    eprintln!("docker stop of a serving taken-over install: {took:?}");
    assert!(app.logs().contains("stopped on request"), "{}", app.logs());
    drop(app);
    left_taken_over(&image, &fixture.volume, &fixture.cipher, &secrets);
}

/// The way back to 2.x through the own-server compose (PLAN-B3 §3), offline (`network_mode: none`): `up -d` takes a
/// format-1 volume over; `docker compose stop`; `docker compose run --rm deltabadger handback` exits 0 with C8 and
/// leaves the lease handed back. 2.70.21's `standalone` then boots on the same volume without a HeldError, serves 200
/// and adopts the bots: its `adopt_handback!` deletes the lease row. Both leases are read under the volume's own keys.
#[test]
fn handback_through_the_compose_gives_the_volume_back_to_2x() {
    let Some(image) = image() else { return };
    let fixture = made_by_2x_migrate(&image);
    let Volume::Named(volume) = &fixture.volume else { unreachable!() };
    let project = Project::new("docker-compose.yml",
        &format!("services:\n  deltabadger:\n    image: {image}\n    network_mode: none\n    ports: !reset []\n\
                  volumes:\n  storage:\n    name: {volume}\n    external: true\n"), &[]);
    project.ok(&["up", "-d"]);
    let app = project.container("deltabadger");
    app.wait_for("/health-check", 200);
    assert!(app.logs().contains("took over ("), "{}", app.logs());
    project.ok(&["stop"]);
    assert_eq!(docker(&["inspect", "-f", "{{.State.ExitCode}}", &app.id]), "0", "{}", app.logs());

    let name = unique("dbhandback");
    let out = exits(project.command(&["run", "--rm", "-T", "--name", &name, "deltabadger", "handback"]), &name);
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert_eq!(out.status.code(), Some(0), "{said}");
    assert!(String::from_utf8_lossy(&out.stdout).contains(C8), "{said}");
    let back = lease(&image, &fixture.volume, &fixture.cipher).expect("a lease");
    assert!(back["engine"] == "none" && back["released_by"] == "rust" && back["handed_back"] == true, "{back}");

    let rails = Container::offline(RAILS_2X, &fixture.volume, &[], &["standalone"]);
    rails.wait_for("/health-check", 200);
    assert!(!rails.logs().contains("HeldError"), "{}", rails.logs());
    docker(&["stop", &rails.id]);
    drop(rails);
    assert_eq!(lease(&image, &fixture.volume, &fixture.cipher), None, "2.70.21 adopted the handback and deleted the row");
}

// ---- B3-7: building from source (PLAN-B3 r1.5) ----

const BUILD_COMPOSE: &str = "docker-compose.build.yml";

/// The test's only change to the build compose: offline (`network_mode: none`, nothing published) and `storage` is
/// `volume`. The build itself still uses the builder's network.
fn offline_on(volume: &Volume) -> String {
    let Volume::Named(name) = volume else { panic!("a named volume") };
    format!("services:\n  deltabadger:\n    network_mode: none\n    ports: !reset []\nvolumes:\n  storage:\n    name: {name}\n    external: true\n")
}

/// `docker compose up -d --build`, as a build-from-source user runs it. Prints how long the build and start took.
fn build_up(project: &Project) {
    let start = Instant::now();
    project.ok(&["up", "-d", "--build"]);
    eprintln!("docker compose up -d --build: {:?}", start.elapsed());
}

/// `docker compose stop`: the command and the service both exit 0.
fn stopped(project: &Project, app: &Container) {
    project.ok(&["stop"]);
    assert_eq!(docker(&["inspect", "-f", "{{.State.ExitCode}}", &app.id]), "0", "{}", app.logs());
}

/// docker-compose.build.yml as it is, on an empty volume: it builds this checkout's Rust program, which sets up a new
/// install (`.secrets` 600, owned by 1000) and serves 200.
#[test]
fn the_build_compose_builds_3_0_and_sets_up_an_empty_volume() {
    let Some(_) = image() else { return };
    let volume = Volume::named();
    let project = Project::new(BUILD_COMPOSE, &offline_on(&volume), &[]);
    build_up(&project);
    let app = project.container("deltabadger");
    app.wait_for("/health-check", 200);
    assert!(app.logs().contains("set up a new install"), "{}", app.logs());
    assert_eq!(app.exec(&["stat", "-c", "%u %a", "/app/storage/.secrets"]), "1000 600");
    let said = app.exec(&["deltabadger", "--version"]);
    assert_eq!(said.lines().next(), Some(format!("deltabadger {}", deltabadger::VERSION).as_str()), "the Rust program of this checkout: {said}");
    stopped(&project, &app);
}

/// docker-compose.build.yml as it is, on a real 2.x volume (format 1): taken over as by the published image.
#[test]
fn the_build_compose_takes_a_2x_volume_over() {
    let Some(image) = image() else { return };
    let fixture = made_by_2x_migrate(&image);
    let secrets = std::fs::read(take(&image, &fixture.volume).path().join(".secrets")).unwrap();
    let project = Project::new(BUILD_COMPOSE, &offline_on(&fixture.volume), &[]);
    build_up(&project);
    let app = project.container("deltabadger");
    app.wait_for("/health-check", 200);
    assert!(app.logs().contains("took over ("), "{}", app.logs());
    stopped(&project, &app);
    left_taken_over(&image, &fixture.volume, &fixture.cipher, &secrets);
}

/// A Behind (2.60.0) install of a build-from-source user, whose SECRET_KEY_BASE is in `.env.docker` beside a copy of
/// docker-compose.build.yml (built from this checkout). 3.0 refuses it: 503, C1 and r4.1's line on stderr, every file
/// byte-equal. B10's command (`compose run` can't swap the image of a service that builds its own) with the same env
/// file, `docker run --rm --env-file .env.docker -v <volume>:/app/storage …:2.70.21 migrate`, then `up -d` → taken over.
#[test]
fn a_behind_install_of_the_build_compose_is_upgraded_by_the_manual_command() {
    let Some(image) = image() else { return };
    let root = format!("{}/..", env!("CARGO_MANIFEST_DIR"));
    let dir = tempfile::tempdir().unwrap();
    let compose = dir.path().join(BUILD_COMPOSE);
    std::fs::copy(format!("{root}/{BUILD_COMPOSE}"), &compose).unwrap();
    std::fs::write(dir.path().join(".env.docker"), "SECRET_KEY_BASE=b3-7-build-secret\n").unwrap();
    let volume = Volume::named();
    let Volume::Named(name) = &volume else { unreachable!() };
    docker(&["run", "--rm", "--network", "none", "-v", &volume.mount(), "-e", "SECRET_KEY_BASE=b3-7-build-secret", RAILS_2_60, "migrate"]);
    let missing = missing_2x(&image, &volume).len();
    let project = Project::layered(&compose.display().to_string(),
        &[&format!("services:\n  deltabadger:\n    build:\n      context: {root}\n"), &offline_on(&volume)], &[]);

    let before = snapshot(&image, &volume);
    build_up(&project);
    let app = project.container("deltabadger");
    app.wait_for("/health-check", 503);
    let said = app.stderr();
    assert!(said.contains("this data is from an older version of Deltabadger") && said.contains(&behind_detail(missing)), "{}", app.logs());
    stopped(&project, &app);
    assert_eq!(snapshot(&image, &volume), before, "the refused start changes no file");

    // B10's line, run where the user's compose file and .env.docker are (offline here: --network none).
    let run = unique("dbmigrate");
    let mut command = Command::new("docker");
    command.current_dir(dir.path()).args(["run", "--rm", "--name", &run, "--network", "none", "--env-file", ".env.docker", "-v",
                                          &format!("{name}:/app/storage"), RAILS_2X, "migrate"]);
    let out = exits(command, &run);
    assert_eq!(out.status.code(), Some(0), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert_eq!(missing_2x(&image, &volume), Vec::<String>::new());

    project.ok(&["up", "-d"]);
    let app = project.container("deltabadger");
    app.wait_for("/health-check", 200);
    assert!(app.logs().contains("took over ("), "{}", app.logs());
    stopped(&project, &app);
}

// ---- B3-6 and B5-2: the Behind guard and the shipped Umbrel app (PLAN-B3 §4 A+; PLAN-B5 §1, §3, §4, §9 item 2) ----

/// The image of Umbrel's `upgrade` step as deltabadger/docker-compose.yml names it: 2.70.21 pinned by its index digest.
const GUARD_IMAGE: &str = "ghcr.io/deltabadger/deltabadger:2.70.21@sha256:7c8f73518134ba6414551b3978f49bd91dacc3ff75d0e1c6aea92401b449de81";
const RAILS_2_34: &str = "ghcr.io/deltabadger/deltabadger:2.34.0";
const RAILS_2_60: &str = "ghcr.io/deltabadger/deltabadger:2.60.0";
const UMBREL_SEED: &str = "b3-6-umbrel-seed";
const UMBREL_SECRET: &str = "SECRET_KEY_BASE=b3-6-umbrel-seed-secret-key-base";
/// The guard's own line when it runs migrate (G1, PLAN-B5 §7).
const GUARD_RUNS: &str = "Deltabadger: your data is from an older version, so it's being updated first.";
/// The guard's line when that migrate fails (F1, PLAN-B5 §7).
const GUARD_FAILED: &str = "Deltabadger couldn't finish updating your data. Nothing was bought or sold. Restart the app to try again; if it \
                            keeps failing, contact support.";

/// The Umbrel app this repository ships (its store's deltabadger/), and the composes installed apps still run: an
/// installed app keeps the compose of its last Update (PLAN-B5 F4, r5.1). 2.34.0's (UMBREL_2_34), 2.60.0's and 2.70.21's
/// (UMBREL_2X), and B5-0's `2.70.21-1` (UMBREL_B5_0: the 2.70.21 compose with both images pinned by digest, r2.1).
const UMBREL: &str = "deltabadger/docker-compose.yml";
const UMBREL_2_34: &str = "rust/tests/fixtures/umbrel/2.34.0-docker-compose.yml";
const UMBREL_B5_0: &str = "rust/tests/fixtures/umbrel/2.70.21-1-docker-compose.yml";

/// umbreld's patch of an app's compose, applied on every install, update and start (PLAN-B5 F5, r1.3, r2.3; umbreld
/// app.ts patchComposeFile): every service gets `container_name: <project>_<service>_1` unless it has one (umbreld's
/// project is the app id, `deltabadger`; a test's own name keeps tests apart), and each volume string has its first
/// `/data/storage/downloads` made `/home/Downloads`, then its first `/data/storage` made `/home` (JavaScript's
/// `String.replace` with a string replaces the first match). 2.0.1 rewrites the source only (the part before the first
/// `:`, `mapServiceVolumeSource`); 1.7.4 rewrites the whole string. The two must agree on every volume a file carries,
/// which is asserted here, so one patch stands for both. So the install lives in `${APP_DATA_DIR}/home`.
/// A `container_name` that is empty or null counts as missing, as umbreld's falsy check does.
fn umbreld_patch(text: &str, project: &str) -> yaml_rust2::Yaml {
    use yaml_rust2::Yaml;
    let mut doc = yaml_rust2::YamlLoader::load_from_str(text).unwrap().remove(0);
    let Yaml::Hash(root) = &mut doc else { panic!("a compose file") };
    let Some(Yaml::Hash(services)) = root.get_mut(&Yaml::String("services".into())) else { panic!("services") };
    for (name, service) in services.iter_mut() {
        let Yaml::Hash(service) = service else { panic!("{name:?}") };
        let container = Yaml::String(format!("{project}_{}_1", name.as_str().unwrap()));
        let key = Yaml::String("container_name".into());
        if !matches!(service.get(&key), Some(Yaml::String(n)) if !n.is_empty()) { service.insert(key, container); }
        let legacy = |s: &str| s.replacen("/data/storage/downloads", "/home/Downloads", 1).replacen("/data/storage", "/home", 1);
        if let Some(Yaml::Array(volumes)) = service.get_mut(&Yaml::String("volumes".into())) {
            for volume in volumes.iter_mut() {
                let Yaml::String(v) = volume else { panic!("{name:?}: a long-form volume; umbreld_patch maps strings only") };
                let source_only = match v.split_once(':') { Some((source, rest)) => format!("{}:{rest}", legacy(source)), None => v.clone() };
                assert_eq!(legacy(v), source_only, "{name:?}: umbrelOS 1.7 and 2.0 would mount {v} differently");
                *v = source_only;
            }
        }
    }
    doc
}

/// The image a pinned reference runs as here: the pin itself, or, on a daemon that can't pull by digest (a Mac whose
/// ghcr pulls are denied), the same image by tag: DELTABADGER_GUARD_IMAGE for 2.70.21's pin, DELTABADGER_UMBREL_WEB_IMAGE
/// for `web`'s 3.0 pin. CI sets neither and pulls the pins.
fn local(pin: &str) -> Option<String> {
    let var = if pin == GUARD_IMAGE { "DELTABADGER_GUARD_IMAGE" } else { "DELTABADGER_UMBREL_WEB_IMAGE" };
    std::env::var(var).ok().filter(|v| !v.trim().is_empty())
}

/// Umbrel's app `file` as umbreld runs it for project `name` (umbreld_patch), offline: `app_proxy` dropped (Umbrel's own),
/// every other service on `network_mode: none`, `images` (service, image) in place of the file's, and a pinned image
/// the daemon can't pull by digest on its `local` tag. `extra` overrides go on top (umbrelOS 2.0's user settings, r3.1).
fn umbrel_app(name: &str, file: &str, images: &[(&str, &str)], extra: &[&str], env: &[(String, String)]) -> Project {
    let text = std::fs::read_to_string(format!("{}/../{file}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let doc = umbreld_patch(&text, name);
    let mut offline = String::from("services:\n  app_proxy: !reset null\n");
    for (service, def) in doc["services"].as_hash().unwrap() {
        let service = service.as_str().unwrap();
        if service == "app_proxy" { continue }
        let pinned = def["image"].as_str().filter(|i| i.contains("@sha256:")).and_then(local);
        let image = images.iter().find(|(s, _)| *s == service).map(|(_, i)| i.to_string()).or(pinned);
        offline += &format!("  {service}:\n    network_mode: none\n");
        if let Some(image) = image { offline += &format!("    image: {image}\n"); }
    }
    let dir = tempfile::tempdir().unwrap();
    let mut patched = String::new();
    yaml_rust2::YamlEmitter::new(&mut patched).dump(&doc).unwrap();
    let mut files = vec![];
    for (i, yaml) in [patched.as_str(), &offline].into_iter().chain(extra.iter().copied()).enumerate() {
        let path = dir.path().join(format!("{i}.yml"));
        std::fs::write(&path, yaml).unwrap();
        files.push(path.display().to_string());
    }
    Project { name: name.into(), files, _overrides: dir, env: env.to_vec() }
}

/// An Umbrel app's data, `APP_DATA_DIR` (a bind, as Umbrel mounts it) with the install in `home` (`storage`), and
/// `project`: the shipped app on it (UMBREL), offline, `web` on the image given or on its pin. Umbrel's variables:
/// APP_SEED, DEVICE_DOMAIN_NAME. Other composes of the same app (`app`) share the project name, as an installed app's
/// files do across its updates.
struct Umbrel { project: Project, storage: Volume, _data: Volume, env: Vec<(String, String)> }

fn umbrel(image: &str) -> Umbrel { Umbrel::new(Some(image)) }

impl Umbrel {
    fn new(web: Option<&str>) -> Self {
        let data = Volume::bind(RAILS_2X);
        std::fs::create_dir_all(data.path().join("home")).unwrap();
        std::fs::create_dir_all(data.path().join("data/logs")).unwrap();
        let storage = Volume::At(format!("{}/home:/app/storage", data.path().display()));
        let env: Vec<(String, String)> = [("APP_DATA_DIR", data.path().display().to_string()), ("APP_SEED", UMBREL_SEED.into()),
                                          ("DEVICE_DOMAIN_NAME", "umbrel.local".into())].into_iter().map(|(k, v)| (k.into(), v)).collect();
        let images: Vec<(&str, &str)> = web.map(|w| ("web", w)).into_iter().collect();
        let project = umbrel_app(&unique("dbumbrel"), UMBREL, &images, &[], &env);
        Umbrel { project, storage, _data: data, env }
    }

    /// Another compose of this app (`file`, `images`, `extra` as `umbrel_app`), under the same project name.
    fn app(&self, file: &str, images: &[(&str, &str)], extra: &[&str]) -> Project { umbrel_app(&self.project.name, file, images, extra, &self.env) }

    /// The container umbreld names `<app>_<service>_1`.
    fn named(&self, service: &str) -> String { format!("{}_{service}_1", self.project.name) }

    /// Umbrel's update (PLAN-B5 F1): `rm --force --stop` with the installed compose `from`, then `up --detach --build`
    /// with `to` (pulling what is missing). A start or restart is the same with `from` = `to`. `up`'s output.
    fn update(&self, from: &Project, to: &Project) -> Output {
        from.ok(&["rm", "--force", "--stop"]);
        to.compose(&["up", "--detach", "--build"])
    }

    /// A 2.x install as Umbrel ran it: `file` (that version's compose) with `rails` on `web` and `jobs`, started as
    /// Umbrel starts it and left running once `web` answers `/up`. 2.34.0's published image can't serve: its Puma
    /// config calls `solid_queue_mode`, which the image's Puma (8.0.2) lacks, so `web` restarts after each start's
    /// migrate. Its install is made by that same image's `migrate` through the same compose (`run --rm web migrate`), and
    /// the app is then left as such a device runs it: `jobs` up, `web` restarting.
    fn install_2x(&self, file: &str, rails: &str) -> Project {
        let old = self.app(file, &[("web", rails), ("jobs", rails)], &[]);
        if rails == RAILS_2_34 { old.ok(&["run", "--rm", "--no-deps", "web", "migrate"]); }
        old.ok(&["up", "--detach", "--build"]);
        if rails != RAILS_2_34 {
            let web = old.container("web");
            web.wait_for("/up", 200);
            std::mem::forget(web); // the app keeps running: Umbrel's update removes it
        }
        old
    }

    /// The step's container (the last one Umbrel ran): its exit code and its whole output.
    fn step(&self) -> (String, String) {
        let name = self.named("upgrade");
        let out = docker_out(&["logs", &name]);
        (docker(&["inspect", "-f", "{{.State.Status}} {{.State.ExitCode}}", &name]),
         format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    }

    /// Umbrel starting the step alone (a fresh container each time), bounded as `exits`: its exit code and its whole output.
    fn guard(&self) -> (Option<i32>, String) {
        self.project.ok(&["rm", "-fs", "upgrade"]);
        let code = exits(self.project.command(&["up", "--exit-code-from", "upgrade", "upgrade"]), &self.named("upgrade")).status.code();
        (code, self.project.ok(&["logs", "--no-color", "--no-log-prefix", "upgrade"]))
    }

    /// `up -d web` (the step first, then `web` once it exited 0): `web`, serving 200.
    fn web_serves(&self) -> Container {
        self.project.ok(&["up", "-d", "web"]);
        let web = self.project.container("web");
        web.wait_for("/health-check", 200);
        web
    }

    /// `web_serves` after taking the install over.
    fn web_takes_over(&self) -> Container {
        let web = self.web_serves();
        assert!(web.logs().contains("took over ("), "{}", web.logs());
        web
    }

    /// A Rails image's command on the install, with Umbrel's secret, offline: its stdout.
    fn rails(&self, rails: &str, command: &[&str]) -> String { self.rails_with(UMBREL_SECRET, rails, command) }

    /// `rails` with `secret` (`SECRET_KEY_BASE=…`) in place of Umbrel's.
    fn rails_with(&self, secret: &str, rails: &str, command: &[&str]) -> String {
        let mount = self.storage.mount();
        let mut args = vec!["run", "--rm", "--network", "none", "-v", &mount, "-e", secret, rails];
        args.extend(command);
        docker(&args)
    }
}

/// 2.70.21's migration versions (its db/migrate) not in the install's schema_migrations.
fn missing_2x(image: &str, storage: &Volume) -> Vec<String> {
    let all = docker(&["run", "--rm", "--network", "none", "--entrypoint", "ls", RAILS_2X, "/app/db/migrate"]);
    let copy = take(image, storage);
    let c = rusqlite::Connection::open(copy.path().join("production.sqlite3")).unwrap();
    let recorded: Vec<String> = c.prepare("SELECT version FROM schema_migrations").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    all.lines().filter_map(|f| f.split('_').next()).filter(|v| !recorded.iter().any(|r| r == v)).map(String::from).collect()
}

/// r2.2's data, through 2.34.0's own models: a user, a Kraken API key, account_transactions on Kraken and Binance (one
/// of them QUICK), a working DCA bot with two orders. Then the queue is emptied, so every job after the step was
/// enqueued by a 2.70.21 migration, not by a seed callback.
const SEED_2_34: &str = r#"
user = User.create!(email: "b3-6@example.test", password: "B3-6-fixture-pass!", name: "Fixture", confirmed_at: Time.current, setup_completed: true)
kraken, binance = Exchange.find_by!(type: "Exchanges::Kraken"), Exchange.find_by!(type: "Exchanges::Binance")
key = ApiKey.create!(user: user, exchange: kraken, key: "b3-6-key", secret: "b3-6-secret", key_type: :trading, status: :correct)
row = ->(exchange, tx, base, amount, quote, quote_amount, at) {
  AccountTransaction.create!(user: user, exchange: exchange, api_key: exchange == kraken ? key : nil, tx_id: tx, entry_type: :buy,
                             base_currency: base, base_amount: amount, quote_currency: quote, quote_amount: quote_amount, transacted_at: at) }
row.(kraken, "b3-6-kraken-1", "BTC", 0.01, "EUR", 300, Time.utc(2025, 1, 10))
row.(binance, "b3-6-binance-1", "QUICK", 100, "USDT", 50, Time.utc(2023, 5, 1))
bot = Bots::DcaSingleAsset.new(user: user, exchange: kraken, label: "B3-6 DCA", status: :scheduled, started_at: Time.utc(2025, 1, 1),
                               settings: { "base_asset_id" => Asset.find_by!(external_id: "bitcoin").id,
                                           "quote_asset_id" => Asset.find_by!(external_id: "EUR.FOREX").id, "quote_amount" => 10.0, "interval" => "day" })
bot.set_missed_quote_amount
bot.save!
2.times { |i| bot.transactions.create!(exchange: kraken, status: :submitted, side: :buy, order_type: :market_order, base: "BTC", quote: "EUR",
                                       external_id: "b3-6-order-#{i}", price: 30000, amount: 0.0003, amount_exec: 0.0003,
                                       quote_amount: 10, quote_amount_exec: 10, created_at: Time.utc(2025, 1, 1 + i)) }
queue = SolidQueue::Record.connection
%w[ready scheduled claimed blocked failed recurring].each { |kind| queue.execute("DELETE FROM solid_queue_#{kind}_executions") }
queue.execute("DELETE FROM solid_queue_jobs")
print "seeded user #{user.id}, queue #{queue.select_value('SELECT COUNT(*) FROM solid_queue_jobs')}"
"#;

/// r2.2's fixture on Umbrel's storage: 2.34.0's `migrate` on the empty folder (with Umbrel's secret in the env, so
/// `.secrets` holds the pair only), then SEED_2_34 through 2.34.0's `rails runner`. The user's id.
fn behind_2_34(u: &Umbrel) -> i64 {
    u.rails(RAILS_2_34, &["migrate"]);
    let said = u.rails(RAILS_2_34, &["rake", "runner", SEED_2_34]);
    let line = said.lines().last().unwrap_or_default();
    assert!(line.ends_with(", queue 0"), "{said}");
    line.trim_start_matches("seeded user ").split(',').next().unwrap().parse().unwrap()
}

/// The step on a Behind install: it says so, runs 2.70.21's migrate (exit 0) and leaves no 2.70.21 migration missing.
fn step_upgrades(image: &str, u: &Umbrel) {
    assert!(!missing_2x(image, &u.storage).is_empty(), "the fixture is Behind");
    let (code, said) = u.guard();
    assert_eq!(code, Some(0), "{said}");
    assert!(said.contains(GUARD_RUNS) && said.contains("Migrations completed!"), "{said}");
    assert_eq!(missing_2x(image, &u.storage), Vec::<String>::new(), "{said}");
}

/// (a) r2.2: a 2.34.0 install with data, upgraded by Umbrel's step and taken over by 3.0. The step's migrate enqueues
/// one Tracker::LedgerJob, one PortfolioSnapshot::BackfillJob and one AccountTransaction::ResolveAssetsJob for the user
/// (Rails' `limits_concurrency … on_conflict: :discard` drops the later LedgerJobs and BackfillJobs, the QUICK refetch's
/// among them, while one is waiting). 3.0's start line counts them; the two wakes are journalled in rust_rails_wakes and
/// run (each job's state shows a run after the wake's time; offline, a run may end in an error); the ResolveAssetsJob is
/// dropped (D-B3-BEHIND-ASSETS-UNRESOLVED): the old ledger rows keep base_asset_id NULL and are listed under their symbol.
#[test]
fn umbrels_step_upgrades_a_2_34_install_and_3_0_takes_its_jobs_over() {
    let Some(image) = image() else { return };
    let u = umbrel(&image);
    let user = behind_2_34(&u);
    step_upgrades(&image, &u);

    let copy = take(&image, &u.storage);
    let queue = rusqlite::Connection::open(copy.path().join("production_queue.sqlite3")).unwrap();
    let jobs: Vec<(String, String)> = queue.prepare("SELECT class_name, arguments FROM solid_queue_jobs ORDER BY class_name").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
    let classes: Vec<&str> = jobs.iter().map(|(class, _)| class.as_str()).collect();
    assert_eq!(classes, ["AccountTransaction::ResolveAssetsJob", "PortfolioSnapshot::BackfillJob", "Tracker::LedgerJob"], "{jobs:?}");
    for (class, arguments) in &jobs {
        let arguments: serde_json::Value = serde_json::from_str(arguments).unwrap();
        assert_eq!(arguments["arguments"], serde_json::json!([user]), "{class}: {arguments}");
    }
    drop((queue, copy));

    let web = u.web_takes_over();
    let took = web.logs().lines().find(|l| l.contains("took over (")).unwrap_or_default().to_string();
    eprintln!("{took}");
    assert!(took.contains("bot(s), 3 queued job(s) removed, 1 of them with nothing left to do"), "{}", web.logs());
    // The two wakes run at this start: each job's state for the user shows a run (ended well or not) after the wake.
    let start = Instant::now();
    let wakes = loop {
        docker(&["pause", &web.id]); // a consistent copy: no write lands between the database and its WAL
        let ran = wakes_ran(&image, &u.storage, user);
        docker(&["unpause", &web.id]);
        if ran.as_ref().is_some_and(|w| w.len() == 2 && w.iter().all(|(_, ran)| *ran)) || start.elapsed() > Duration::from_secs(90) { break ran; }
        std::thread::sleep(Duration::from_secs(2));
    };
    assert_eq!(wakes, Some(vec![("portfolio_backfill".to_string(), true), ("tracker_ledger".to_string(), true)]), "{}", web.logs());
    assert_eq!(web.stop(), "0", "{}", web.logs());

    let copy = take(&image, &u.storage);
    let queue = rusqlite::Connection::open(copy.path().join("production_queue.sqlite3")).unwrap();
    let left: i64 = queue.query_row("SELECT COUNT(*) FROM solid_queue_jobs WHERE class_name IN ('AccountTransaction::ResolveAssetsJob', \
        'PortfolioSnapshot::BackfillJob', 'Tracker::LedgerJob')", [], |r| r.get(0)).unwrap();
    assert_eq!(left, 0, "3.0 took the three rows: two as wakes, ResolveAssetsJob dropped");
    let primary = rusqlite::Connection::open(copy.path().join("production.sqlite3")).unwrap();
    let rows: Vec<(String, Option<i64>)> = primary.prepare("SELECT base_currency, base_asset_id FROM account_transactions ORDER BY id").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
    // Listed under their symbols: the tracker reads a row without base_asset_id by its symbol, as Rails does; its list is
    // proven on such rows in-process (tests/tracker_api.rs seeds every row without one). Not here: an App needs the web
    // assets, which this job's test build doesn't have.
    assert_eq!(rows, [("BTC".to_string(), None), ("QUICK".to_string(), None)], "D-B3-BEHIND-ASSETS-UNRESOLVED");
}

/// The journal's wakes for `user`, sorted by job, each with whether its job state shows a run (last_run_at or
/// last_error_at) at or after the wake's time. None until the journal is there.
fn wakes_ran(image: &str, storage: &Volume, user: i64) -> Option<Vec<(String, bool)>> {
    let copy = take(image, storage);
    let c = rusqlite::Connection::open(copy.path().join("production.sqlite3")).ok()?;
    let value = |key: &str| c.query_row("SELECT value FROM app_configs WHERE key = ?1", [key], |r| r.get::<_, String>(0)).ok();
    let wakes: Vec<serde_json::Value> = serde_json::from_str(&value("rust_rails_wakes")?).ok()?;
    let mut out: Vec<(String, bool)> = wakes.iter().map(|w| {
        assert_eq!(w["scope"], user.to_string(), "{w}");
        let job = w["job"].as_str().unwrap().to_string();
        let at = w["at"].as_str().unwrap();
        let state: serde_json::Value = value(&format!("rust_job.{job}:{user}")).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let ran = ["last_run_at", "last_error_at"].iter().any(|k| state[*k].as_str().is_some_and(|t| t >= at));
        (job, ran)
    }).collect();
    out.sort();
    Some(out)
}

/// (b) The late tail: a 2.60.0 install, upgraded by the step and taken over.
#[test]
fn umbrels_step_upgrades_a_2_60_install() {
    let Some(image) = image() else { return };
    let u = umbrel(&image);
    u.rails(RAILS_2_60, &["migrate"]);
    step_upgrades(&image, &u);
    let web = u.web_takes_over();
    assert_eq!(web.stop(), "0", "{}", web.logs());
}

/// (c) r3.2: a partly upgraded install, as a failed 2.x update leaves it (the 2.34.0 fixture taken by 2.70.21 up to
/// 20260828094446, just before the first migration that disables its transaction): the step sees it Behind, completes
/// it, and 3.0 takes it over. `db:migrate:primary`, not r3.2's `db:migrate`: with several databases Rails' `db:migrate
/// VERSION=` runs that one migration alone. An upgrade killed inside a non-transactional migration is
/// R-B3-BEHIND-NONTX, not proved here.
#[test]
fn umbrels_step_completes_a_partly_upgraded_install() {
    let Some(image) = image() else { return };
    let u = umbrel(&image);
    behind_2_34(&u);
    u.rails(RAILS_2X, &["rake", "db:migrate:primary", "VERSION=20260828094446"]);
    let missing = missing_2x(&image, &u.storage);
    assert!(!missing.is_empty() && !missing.iter().any(|v| v.as_str() <= "20260828094446"), "partly upgraded: {missing:?}");
    step_upgrades(&image, &u);
    let web = u.web_takes_over();
    assert_eq!(web.stop(), "0", "{}", web.logs());
}

/// The step on an install it must leave alone: exit 0, no output at all (so no Rails boot and no migrate), every file
/// byte-equal (`snapshot`: no new row anywhere).
fn step_leaves_alone(image: &str, u: &Umbrel, what: &str) {
    let before = snapshot(image, &u.storage);
    let (code, said) = u.guard();
    assert_eq!((code, said.as_str()), (Some(0), ""), "{what}");
    assert_eq!(snapshot(image, &u.storage), before, "{what}: the step changes no file");
}

/// (d) The step is a no-op on every install that isn't Behind: an empty folder (no install is created: no primary,
/// no `.secrets`), a 3.0-created install, a 2.70.21 install at the frozen schema, the same install taken over by 3.0,
/// and handed back to 2.x (B3-4's handback). A Rails boot would refuse the taken-over one and adopt the handed-back one.
#[test]
fn umbrels_step_leaves_every_install_that_is_not_behind_alone() {
    let Some(image) = image() else { return };
    let u = umbrel(&image);
    step_leaves_alone(&image, &u, "empty");
    assert_eq!(snapshot(&image, &u.storage), ".", "an empty folder stays empty");
    let web = u.web_serves();
    assert!(web.logs().contains("set up a new install"), "{}", web.logs());
    assert_eq!(web.stop(), "0", "{}", web.logs());
    step_leaves_alone(&image, &u, "3.0-created");

    let u = umbrel(&image);
    u.rails(RAILS_2X, &["migrate"]);
    step_leaves_alone(&image, &u, "2.70.21, at the frozen schema");
    let web = u.web_takes_over();
    assert_eq!(web.stop(), "0", "{}", web.logs());
    step_leaves_alone(&image, &u, "taken over");
    let name = unique("dbhandback");
    let out = exits(u.project.command(&["run", "--rm", "-T", "--no-deps", "--name", &name, "web", "handback"]), &name);
    assert_eq!(out.status.code(), Some(0), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains(C8));
    step_leaves_alone(&image, &u, "handed back");
}

/// C1-U (PLAN-B5 r2.2, r6 Q9' A): Umbrel's 2.x compose as installed (UMBREL_2X: no step; its `web` sets
/// DELTABADGER_PLATFORM: umbrel) with `web` on 3.0, on a Behind 2.60.0 install. `web` refuses with 503 in Umbrel's words (not generic C1), the
/// operator's detail on stderr, and every file stays byte-equal.
#[test]
fn umbrels_web_without_the_step_refuses_a_behind_install_in_umbrels_words() {
    let Some(image) = image() else { return };
    let u = umbrel(&image);
    let old = u.app(UMBREL_2X, &[("web", &image)], &[]);
    u.rails(RAILS_2_60, &["migrate"]);
    let missing = missing_2x(&image, &u.storage).len();
    let before = snapshot(&image, &u.storage);
    old.ok(&["up", "-d", "web"]);
    let web = old.container("web");
    web.wait_for("/health-check", 503);
    let page = web.exec(&["curl", "-s", "http://localhost:3000/"]);
    assert!(page.contains(&on_page(C1_UMBREL)) && !page.contains("Go back to your previous"), "{page}");
    assert!(web.stderr().contains(&format!("deltabadger: {C1_UMBREL}\ndeltabadger: {}", behind_detail(missing))), "{}", web.logs());
    assert_eq!(web.stop(), "0", "{}", web.logs());
    assert_eq!(snapshot(&image, &u.storage), before, "a refused start changes no file");
}

/// An image `FROM` the guard image with 2.70.21's last migration replaced by one that raises (b3-6-pr mutation 4).
/// Removed on drop.
struct FailingGuard { tag: String, last: String }

impl FailingGuard {
    fn build() -> Self {
        let base = std::env::var("DELTABADGER_GUARD_IMAGE").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| GUARD_IMAGE.into());
        let files = docker(&["run", "--rm", "--network", "none", "--entrypoint", "ls", &base, "/app/db/migrate"]);
        let file = files.lines().max().expect("2.70.21 has migrations").to_string();
        let original = docker(&["run", "--rm", "--network", "none", "--entrypoint", "cat", &base, &format!("/app/db/migrate/{file}")]);
        let class = original.lines().find(|l| l.starts_with("class ")).expect("a migration class").to_string();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(&file), format!("{class}\n  def up\n    raise \"B5-3: this migration fails on purpose\"\n  end\nend\n")).unwrap();
        std::fs::write(dir.path().join("Dockerfile"), format!("FROM {base}\nCOPY {file} /app/db/migrate/{file}\n")).unwrap();
        let tag = format!("{}:failing", unique("dbguard"));
        docker(&["build", "-q", "-t", &tag, &dir.path().display().to_string()]);
        FailingGuard { tag, last: file.split('_').next().unwrap().to_string() }
    }
}

impl Drop for FailingGuard {
    fn drop(&mut self) { let _ = Command::new("docker").args(["rmi", "-f", &self.tag]).output(); }
}

/// What 3.0 would have written on the install: its tables (`rust_*`), its lease row, its backups.
fn traces_of_3_0(image: &str, storage: &Volume) -> (Vec<String>, Option<String>, bool) {
    let copy = take(image, storage);
    let c = rusqlite::Connection::open(copy.path().join("production.sqlite3")).unwrap();
    let tables = c.prepare("SELECT name FROM sqlite_master WHERE name LIKE 'rust\\_%' ESCAPE '\\'").unwrap()
        .query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    let lease = c.query_row("SELECT value FROM app_configs WHERE key = ?1", [deltabadger::lease::KEY], |r| r.get(0)).ok();
    (tables, lease, copy.path().join("backups").exists())
}

/// F1 (PLAN-B5 §3.2, §7; b3-6-pr mutation 4 as a test): Umbrel's step with a migrate that fails, on a 2.60.0 install.
/// `up -d web` fails naming `upgrade`; `web` is created and never started; the step's log has G1 and the F1 line; 3.0
/// wrote nothing (no `rust_*` table, no lease row, no backup); the install keeps the failed migration missing. Then the
/// real guard image: the step completes and `web` takes over, 200.
#[test]
fn a_failing_step_keeps_web_stopped_says_so_and_the_next_start_completes_it() {
    let Some(image) = image() else { return };
    let failing = FailingGuard::build();
    let u = umbrel(&image);
    u.rails(RAILS_2_60, &["migrate"]);
    assert!(!missing_2x(&image, &u.storage).is_empty(), "the fixture is Behind");
    let untouched = traces_of_3_0(&image, &u.storage);
    assert_eq!((untouched.0.as_slice(), untouched.2), (&[][..], false), "a 2.60.0 install has nothing of 3.0's");

    let broken = u.app(UMBREL, &[("web", &image), ("upgrade", &failing.tag)], &[]);
    let web_name = u.named("web");
    let out = exits(broken.command(&["up", "-d", "web"]), &web_name);
    let up = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(!out.status.success() && up.contains("\"upgrade\" didn't complete successfully"), "{up}");
    assert_eq!(docker(&["inspect", "-f", "{{.State.Status}} {{.State.StartedAt}}", &web_name]), "created 0001-01-01T00:00:00Z", "web never started");
    let said = broken.ok(&["logs", "--no-color", "--no-log-prefix", "upgrade"]);
    assert!(said.contains(GUARD_RUNS) && said.contains(GUARD_FAILED) && !said.contains("Migrations completed!"), "{said}");
    assert_eq!(traces_of_3_0(&image, &u.storage), untouched, "3.0 never ran: no rust_* table, no lease, no backup");
    assert_eq!(missing_2x(&image, &u.storage), std::slice::from_ref(&failing.last), "the failed migration stays to do");
    drop(broken);

    let web = u.web_takes_over();
    let said = u.project.ok(&["logs", "--no-color", "--no-log-prefix", "upgrade"]);
    assert!(said.contains(GUARD_RUNS) && said.contains("Migrations completed!") && !said.contains(GUARD_FAILED), "{said}");
    assert_eq!(missing_2x(&image, &u.storage), Vec::<String>::new());
    assert_eq!(web.stop(), "0", "{}", web.logs());
}

/// (e) r1.1, the own-server manual command (B10), on the own-server compose copied into a folder of its own with
/// SECRET_KEY_BASE in `.env.docker` beside it, offline. A 2.60.0 install: 3.0 refuses it (503, C1 and r4.1's line on
/// stderr, nothing changed); then B10's steps exactly: `docker compose stop`, the compose file's image set to
/// 2.70.21, `docker compose run --rm deltabadger migrate`, the image set back, `docker compose up -d` → taken over, 200.
#[test]
fn the_own_server_manual_command_upgrades_a_behind_install() {
    let Some(image) = image() else { return };
    let dir = tempfile::tempdir().unwrap();
    let compose = dir.path().join("docker-compose.yml");
    let shipped = std::fs::read_to_string(format!("{}/../docker-compose.yml", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let with_image = |to: &str| std::fs::write(&compose, shipped.replace("image: ghcr.io/deltabadger/deltabadger:latest", &format!("image: {to}"))).unwrap();
    std::fs::write(dir.path().join(".env.docker"), "SECRET_KEY_BASE=b3-6-own-server-secret\n").unwrap();
    let volume = Volume::named();
    let Volume::Named(name) = &volume else { unreachable!() };
    docker(&["run", "--rm", "--network", "none", "-v", &volume.mount(), "-e", "SECRET_KEY_BASE=b3-6-own-server-secret", RAILS_2_60, "migrate"]);
    let missing = missing_2x(&image, &volume).len();
    let project = Project::layered(&compose.display().to_string(),
        &[&format!("services:\n  deltabadger:\n    network_mode: none\n    ports: !reset []\nvolumes:\n  storage:\n    name: {name}\n    external: true\n")], &[]);

    with_image(&image);
    let before = snapshot(&image, &volume);
    project.ok(&["up", "-d"]);
    let app = project.container("deltabadger");
    app.wait_for("/health-check", 503);
    let said = app.stderr();
    assert!(said.contains(&format!("deltabadger: {C1}\ndeltabadger: {}", behind_detail(missing))), "{}", app.logs());
    project.ok(&["stop"]);
    assert_eq!(snapshot(&image, &volume), before, "the refused start changes no file");

    with_image(RAILS_2X);
    let run = unique("dbmigrate");
    let out = exits(project.command(&["run", "--rm", "-T", "--name", &run, "deltabadger", "migrate"]), &run);
    assert_eq!(out.status.code(), Some(0), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert_eq!(missing_2x(&image, &volume), Vec::<String>::new());

    with_image(&image);
    project.ok(&["up", "-d"]);
    let app = project.container("deltabadger");
    app.wait_for("/health-check", 200);
    assert!(app.logs().contains("took over ("), "{}", app.logs());
    project.ok(&["stop"]);
    drop(project);
    drop(volume);
}

/// The first-run setup form of a new install, posted the way a browser that opened Umbrel's app at `host` posts it:
/// GET /setup for the session cookie and the form's token, then the POST, both with `Host: host` (app_proxy forwards
/// the browser's own Host, PLAN-B5 §0 F9) and the POST with `Origin: http://host`. Asked from inside `web` (offline).
/// The POST's status and its Location.
fn setup_posted_from(web: &Container, host: &str) -> (u16, String) {
    let script = r#"curl -s -c /tmp/jar -H "Host: $1" http://localhost:3000/setup | grep -o 'name="authenticity_token" value="[^"]*"' | head -1 | cut -d'"' -f4 > /tmp/token
        curl -s -o /dev/null -D /tmp/head -w '%{http_code}\n' -b /tmp/jar -H "Host: $1" -H "Origin: http://$1" --data-urlencode "authenticity_token=$(cat /tmp/token)" \
             --data-urlencode 'user[name]=Origin' --data-urlencode 'user[email]=origin@example.com' --data-urlencode 'user[password]=Origin-password-1234' \
             http://localhost:3000/setup
        grep -i '^location:' /tmp/head | cut -d' ' -f2 | tr -d '\r'"#;
    let said = web.exec(&["sh", "-c", script, "sh", host]);
    let (status, location) = said.split_once('\n').unwrap_or((&said, ""));
    (status.parse().unwrap_or_else(|_| panic!("{said}")), location.to_string())
}

/// `web` on the 2.x Umbrel compose (UMBREL_2X) with `web_image`, on a new install (an empty APP_DATA_DIR, Umbrel's
/// APP_SEED and DEVICE_DOMAIN_NAME, so APP_ROOT_URL is http://umbrel.local:3737), offline: `setup_posted_from(host)`.
fn umbrel_setup_from(image: &str, web_image: &str, host: &str) -> (u16, String) {
    let data = Volume::bind(image);
    std::fs::create_dir_all(data.path().join("data/storage")).unwrap();
    std::fs::create_dir_all(data.path().join("data/logs")).unwrap();
    let offline = format!("services:\n  app_proxy: !reset null\n  web:\n    image: {web_image}\n    network_mode: none\n");
    let app_data_dir = data.path().display().to_string();
    let project = Project::new(UMBREL_2X, &offline, &[("APP_DATA_DIR", &app_data_dir), ("APP_SEED", UMBREL_SEED), ("DEVICE_DOMAIN_NAME", "umbrel.local")]);
    project.ok(&["up", "-d", "web"]);
    let web = project.container("web");
    web.wait_for("/setup", 200);
    setup_posted_from(&web, host)
}

/// R-B5-ORIGIN (PLAN-B5 §1.4): Umbrel users open the app at the name APP_ROOT_URL carries (umbrel.local:3737) or at the
/// device's address (192.168.1.10:3737 here) or another name. 2.70.21 (the oracle) accepts a form from either: Rails
/// compares `Origin` with the request's own base URL. 3.0 accepts both too (D-B5-ORIGIN-HOST): APP_ROOT_URL's origin or
/// the request's own. Accepted = the admin is created and the browser sent on to /bots.
#[test]
fn umbrels_web_accepts_a_form_from_the_address_the_browser_used() {
    let Some(image) = image() else { return };
    for web in [RAILS_2X, image.as_str()] {
        for host in ["umbrel.local:3737", "192.168.1.10:3737"] {
            let (status, location) = umbrel_setup_from(&image, web, host);
            eprintln!("{web} from {host}: {status} {location}");
            assert!(status == 302 && location.ends_with("/bots"), "{web} from {host}: {status} {location}");
        }
    }
}

// ---- B5-2: the shipped Umbrel app, through Umbrel's own steps (PLAN-B5 §9 item 2, r1.1, r1.2, r2.1, r2.2, r3.1,
// r4.1, r5.1, r5.2). Every proof runs deltabadger/docker-compose.yml and the frozen 2.x composes through umbreld's
// patch (`umbrel_app`), offline; a 2.x install is made by its own 2.x image through its own version's compose. ----

/// `up`'s output, which must have succeeded.
fn up_ok(out: &Output) { assert!(out.status.success(), "up: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)) }

/// The step ran and left the install alone: exited 0 and said nothing.
fn step_silent(u: &Umbrel) { assert_eq!(u.step(), ("exited 0".to_string(), String::new())) }

/// The step ran and brought the install up to date: G1, 2.70.21's migrate, exit 0, nothing missing.
fn step_migrated(image: &str, u: &Umbrel) {
    let (state, said) = u.step();
    assert_eq!(state, "exited 0", "{said}");
    assert!(said.contains(GUARD_RUNS) && said.contains("Migrations completed!") && !said.contains(GUARD_FAILED), "{said}");
    assert_eq!(missing_2x(image, &u.storage), Vec::<String>::new(), "{said}");
}

/// No `jobs` container of this app exists (Umbrel's update removed it with the old compose; nothing re-creates it).
fn no_jobs(u: &Umbrel) { assert_eq!(docker(&["ps", "-a", "-q", "--filter", &format!("name=^{}$", u.named("jobs"))]), "", "jobs is gone") }

/// The host folder `container` mounts at /app/storage.
fn storage_source(container: &str) -> String {
    docker(&["inspect", "-f", "{{range .Mounts}}{{if eq .Destination \"/app/storage\"}}{{.Source}}{{end}}{{end}}", container])
}

/// The install's backups/ entries.
fn backups(image: &str, storage: &Volume) -> Vec<String> {
    std::fs::read_dir(take(image, storage).path().join("backups")).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default()
}

/// The install's `.secrets` and the keys 2.x encrypted it under (Umbrel's secret in the env, as both composes set it).
fn umbrel_keys(image: &str, u: &Umbrel) -> (Vec<u8>, Cipher) {
    let copy = take(image, &u.storage);
    (std::fs::read(copy.path().join(".secrets")).unwrap(), Cipher::new(&keys_2x(copy.path(), &[UMBREL_SECRET]).1))
}

/// Waits up to 30 s for `container`'s log to hold `line`.
fn logs_soon(container: &Container, line: &str) {
    let start = Instant::now();
    while !container.logs().contains(line) && start.elapsed() < Duration::from_secs(30) { std::thread::sleep(Duration::from_millis(500)); }
    assert!(container.logs().contains(line), "{line}\n{}", container.logs());
}

/// Umbrel's stop of the running app (`rm --force --stop` with `project`, no `-t`): `web`'s exit code and whole log, read
/// while the stop removes the container, and how long the stop took.
fn umbrel_stops(project: &Project, web: &Container) -> (String, String, Duration) {
    let wait = Command::new("docker").args(["wait", &web.id]).stdout(Stdio::piped()).spawn().unwrap();
    let logs = Command::new("docker").args(["logs", "-f", &web.id]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    std::thread::sleep(Duration::from_secs(1));
    let start = Instant::now();
    project.ok(&["rm", "--force", "--stop"]);
    let took = start.elapsed();
    let code = String::from_utf8(wait.wait_with_output().unwrap().stdout).unwrap().trim().to_string();
    let logs = logs.wait_with_output().unwrap();
    (code, format!("{}{}", String::from_utf8_lossy(&logs.stdout), String::from_utf8_lossy(&logs.stderr)), took)
}

/// Fresh (§9 item 2, §4): the shipped app on an empty APP_DATA_DIR as Umbrel installs it (`up --detach --build`), with
/// `web` on its pin and then on this PR's build. The step exits 0 saying nothing; `web` sets up a new install and
/// serves; one engine runs (in `web`) and there is no `jobs`; `upgrade` and `web` mount the same host folder,
/// `${APP_DATA_DIR}/home`. Umbrel's stop gives `web` its 60 s (StopTimeout) and `web` stops on request with 0.
#[test]
fn umbrels_fresh_install_runs_one_engine_and_stops_on_request() {
    let Some(image) = image() else { return };
    for web_image in [None, Some(image.as_str())] {
        let u = Umbrel::new(web_image);
        up_ok(&u.project.compose(&["up", "--detach", "--build"]));
        let web = u.project.container("web");
        web.wait_for("/health-check", 200);
        assert_eq!(web.status("/setup"), Some(200), "{web_image:?}");
        assert!(web.logs().contains("set up a new install"), "{}", web.logs());
        step_silent(&u);
        no_jobs(&u);
        let engines: usize = u.project.ok(&["ps", "-q"]).lines()
            .map(|id| docker(&["top", id, "-o", "pid,comm"]).lines().filter(|l| l.split_whitespace().nth(1) == Some("deltabadger")).count()).sum();
        assert_eq!(engines, 1, "one deltabadger process across the app's containers");
        let home = storage_source(&u.named("web"));
        assert!(home.ends_with("/home"), "{home}");
        assert_eq!(storage_source(&u.named("upgrade")), home);
        assert_eq!(docker(&["inspect", "-f", "{{.Config.StopTimeout}}", &web.id]), "60", "stop_grace_period");

        let (code, logs, took) = umbrel_stops(&u.project, &web);
        eprintln!("{web_image:?}: Umbrel's stop of a serving web took {took:?}");
        assert!(code == "0" && logs.contains("stopped on request"), "{code}\n{logs}");
    }
}

/// A 2.70.21 install's Umbrel update (§9 item 2, r1.3), `web` on the shipped pin. The install is made by 2.70.21 through
/// its own compose (UMBREL_2X) and running when the user clicks Update. The update removes `jobs` (the stop runs the old
/// file), the step says nothing, 3.0 takes the install over once with one backup, and old `web`, old `jobs`, `upgrade`
/// and new `web` mount the same host folder. Then, on the install 3.0 owns: Umbrel's restart (the step silent again, no
/// second backup, no takeover from Rails); a power loss (`docker kill` of every container, then the boot's
/// `up --detach --build`: the step runs again, silent, and 3.0 continues AfterCrash); and a later 3.x update (the same
/// file, `web` on this PR's build): no new backup, 200, `.secrets` byte-equal and the lease `rust`.
#[test]
fn umbrels_update_takes_a_2_70_21_install_over_and_later_starts_keep_it() {
    let Some(image) = image() else { return };
    let u = Umbrel::new(None);
    let old = u.install_2x(UMBREL_2X, RAILS_2X);
    let home = storage_source(&u.named("web"));
    assert!(home.ends_with("/home"), "{home}");
    assert_eq!(storage_source(&u.named("jobs")), home);
    let (secrets, cipher) = umbrel_keys(&image, &u);

    up_ok(&u.update(&old, &u.project));
    let web = u.project.container("web");
    web.wait_for("/health-check", 200);
    no_jobs(&u);
    step_silent(&u);
    assert!(web.logs().matches("took over (").count() == 1 && web.logs().contains("took over (FromRails)"), "{}", web.logs());
    for service in ["upgrade", "web"] { assert_eq!(storage_source(&u.named(service)), home, "{service}"); }
    assert_eq!(backups(&image, &u.storage).len(), 1);

    // Umbrel's restart of the app.
    up_ok(&u.update(&u.project, &u.project));
    let web = u.project.container("web");
    web.wait_for("/health-check", 200);
    step_silent(&u);
    assert!(!web.logs().contains("took over (FromRails)"), "{}", web.logs());
    assert_eq!(backups(&image, &u.storage).len(), 1, "no second backup");

    // A power loss, then the boot's start.
    let started = docker(&["inspect", "-f", "{{.State.StartedAt}}", &u.named("upgrade")]);
    for id in u.project.ok(&["ps", "-q"]).lines() { docker(&["kill", id]); }
    up_ok(&u.project.compose(&["up", "--detach", "--build"]));
    let web = u.project.container("web");
    web.wait_for("/health-check", 200);
    assert_ne!(docker(&["inspect", "-f", "{{.State.StartedAt}}", &u.named("upgrade")]), started, "the step ran again");
    step_silent(&u);
    assert!(web.logs().contains("took over (AfterCrash)"), "{}", web.logs());
    assert_eq!(backups(&image, &u.storage).len(), 1, "no second backup");

    // A later 3.x update: the same app with `web` on another image.
    let next = u.app(UMBREL, &[("web", &image)], &[]);
    up_ok(&u.update(&u.project, &next));
    let web = next.container("web");
    web.wait_for("/health-check", 200);
    assert_eq!(docker(&["inspect", "-f", "{{.Config.Image}}", &web.id]), image);
    step_silent(&u);
    assert!(!web.logs().contains("took over (FromRails)"), "{}", web.logs());
    next.ok(&["stop"]);
    assert_eq!(docker(&["inspect", "-f", "{{.State.ExitCode}}", &web.id]), "0", "{}", web.logs());
    left_taken_over(&image, &u.storage, &cipher, &secrets);
}

/// A Behind install's Umbrel update (§9 item 2), `web` on the shipped pin and then on this PR's build: made by `rails`
/// through `file`, its own version's compose (with SEED_2_34's data when `seed`), running when the user clicks Update.
/// The step brings the data up to date, `jobs` is gone, and 3.0 takes it over: one backup, lease `rust`, `.secrets`
/// byte-equal.
fn behind_update(image: &str, file: &str, rails: &str, seed: bool) {
    for web_image in [None, Some(image)] {
        let u = Umbrel::new(web_image);
        let old = u.install_2x(file, rails);
        if seed {
            old.ok(&["stop"]);
            let said = u.rails(rails, &["rake", "runner", SEED_2_34]);
            assert!(said.ends_with(", queue 0"), "{said}");
            up_ok(&old.compose(&["up", "--detach", "--build"]));
        }
        assert!(!missing_2x(image, &u.storage).is_empty(), "the fixture is Behind");
        let (secrets, cipher) = umbrel_keys(image, &u);
        up_ok(&u.update(&old, &u.project));
        let web = u.project.container("web");
        web.wait_for("/health-check", 200);
        step_migrated(image, &u);
        no_jobs(&u);
        assert!(web.logs().contains("took over (FromRails)"), "{}", web.logs());
        u.project.ok(&["stop"]);
        left_taken_over(image, &u.storage, &cipher, &secrets);
    }
}

#[test]
fn umbrels_update_upgrades_a_behind_2_34_install() {
    let Some(image) = image() else { return };
    behind_update(&image, UMBREL_2_34, RAILS_2_34, true);
}

#[test]
fn umbrels_update_upgrades_a_behind_2_60_install() {
    let Some(image) = image() else { return };
    behind_update(&image, UMBREL_2X, RAILS_2_60, false);
}

/// r1.1 (a) and r2.1: `:latest` = 3.0 reaching a 2.70.21 install under its 2.x compose (UMBREL_2X with both images on
/// this PR's build: what a re-pull of a moved `:latest` gives) takes it over, `jobs` idles (J1). Then the wrong order
/// r2.1 rules out, as a store still offering B5-0 would give it: an Umbrel update to B5-0's compose (2.70.21 pinned)
/// fails, its `web` refusing the install 3.0 owns, and nothing is written but Rails' boot files. The update to the
/// shipped app then recovers it: the step says nothing, 3.0 serves, no new backup, lease `rust`.
#[test]
fn umbrels_latest_3_0_takes_a_2_70_21_install_over_and_b5_0_after_it_changes_nothing() {
    let Some(image) = image() else { return };
    let u = umbrel(&image);
    let old = u.install_2x(UMBREL_2X, RAILS_2X);
    let (secrets, cipher) = umbrel_keys(&image, &u);
    let latest = u.app(UMBREL_2X, &[("web", &image), ("jobs", &image)], &[]);
    up_ok(&u.update(&old, &latest));
    let web = latest.container("web");
    web.wait_for("/health-check", 200);
    assert!(web.logs().contains("took over (FromRails)"), "{}", web.logs());
    let jobs = latest.container("jobs");
    logs_soon(&jobs, J1);
    latest.ok(&["stop"]);
    let before = snapshot(&image, &u.storage);

    let b5_0 = u.app(UMBREL_B5_0, &[], &[]);
    let out = u.update(&latest, &b5_0);
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    eprintln!("B5-0's up on a 3.0-owned install: {said}");
    assert!(!out.status.success(), "{said}");
    let rails = docker_out(&["logs", &u.named("web")]);
    let rails = format!("{}{}", String::from_utf8_lossy(&rails.stdout), String::from_utf8_lossy(&rails.stderr));
    assert!(rails.contains("still owns this data"), "{rails}");
    b5_0.ok(&["stop"]);
    assert_eq!(snapshot(&image, &u.storage), before, "B5-0's 2.70.21 refuses the install and changes nothing");

    up_ok(&u.update(&b5_0, &u.project));
    let web = u.project.container("web");
    web.wait_for("/health-check", 200);
    step_silent(&u);
    assert!(!web.logs().contains("took over (FromRails)"), "{}", web.logs());
    u.project.ok(&["stop"]);
    left_taken_over(&image, &u.storage, &cipher, &secrets);
}

/// r1.1 (b), r2.2, r5.1: `:latest` = 3.0 reaching a Behind install under its own 2.x compose (both images on this PR's
/// build). 3.0 refuses it untouched with 503: under 2.70.21's compose (DELTABADGER_PLATFORM set, `jobs` waits for
/// `web`'s health) `up` fails on that health and the page is C1-U; under 2.34.0's (no marker, `jobs` waits for nothing)
/// `up` succeeds, the page is generic C1 and `jobs` idles (J1). Either way every file stays byte-equal, and the Umbrel
/// update to the shipped app then runs the step and 3.0 takes the install over.
fn latest_refuses_a_behind_install_until_the_update(image: &str, file: &str, rails: &str) {
    let marker = file == UMBREL_2X;
    let u = umbrel(image);
    let old = u.install_2x(file, rails);
    old.ok(&["stop"]);
    let before = snapshot(image, &u.storage);
    let latest = u.app(file, &[("web", image), ("jobs", image)], &[]);
    let out = u.update(&old, &latest);
    let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    eprintln!("{file}: up on a Behind install: {said}");
    assert_eq!(out.status.success(), !marker, "{said}");
    if marker { assert!(said.contains("is unhealthy"), "{said}"); }
    let web = latest.container("web");
    web.wait_for("/health-check", 503);
    let page = web.exec(&["curl", "-s", "http://localhost:3000/"]);
    assert!(page.contains(&on_page(if marker { C1_UMBREL } else { C1 })), "{page}");
    let jobs = (!marker).then(|| latest.container("jobs"));
    if let Some(jobs) = &jobs { logs_soon(jobs, J1); }
    latest.ok(&["stop"]);
    assert_eq!(snapshot(image, &u.storage), before, "a refused start changes no file");

    up_ok(&u.update(&latest, &u.project));
    let web = u.project.container("web");
    web.wait_for("/health-check", 200);
    step_migrated(image, &u);
    no_jobs(&u);
    assert!(web.logs().contains("took over (FromRails)"), "{}", web.logs());
}

#[test]
fn umbrels_latest_3_0_refuses_a_2_60_install_in_umbrels_words_until_the_update() {
    let Some(image) = image() else { return };
    latest_refuses_a_behind_install_until_the_update(&image, UMBREL_2X, RAILS_2_60);
}

#[test]
fn umbrels_latest_3_0_refuses_a_2_34_install_under_its_own_compose_until_the_update() {
    let Some(image) = image() else { return };
    latest_refuses_a_behind_install_until_the_update(&image, UMBREL_2_34, RAILS_2_34);
}

/// r1.2: an Umbrel update whose pull fails (`upgrade`'s image unresolvable: a registry that isn't there), on a 2.60.0
/// install. `up` fails, the old containers are already gone (the stop ran first) and every file is byte-equal. The next
/// start (`up` with the compose the update installed, its images resolvable) runs the step and 3.0 takes the install over.
#[test]
fn umbrels_update_whose_pull_fails_leaves_the_install_and_the_next_start_completes_it() {
    let Some(image) = image() else { return };
    let u = umbrel(&image);
    let old = u.install_2x(UMBREL_2X, RAILS_2_60);
    old.ok(&["stop"]);
    let before = snapshot(&image, &u.storage);
    let unresolvable = format!("127.0.0.1:9/deltabadger/deltabadger:2.70.21@sha256:{}", "0".repeat(64));
    let failing = u.app(UMBREL, &[("web", &image), ("upgrade", &unresolvable)], &[]);
    let out = u.update(&old, &failing);
    assert!(!out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(failing.ok(&["ps", "-a", "-q"]), "", "no container of the app is left");
    assert_eq!(snapshot(&image, &u.storage), before, "a failed pull changes no file");

    up_ok(&u.project.compose(&["up", "--detach", "--build"]));
    let web = u.project.container("web");
    web.wait_for("/health-check", 200);
    step_migrated(&image, &u);
    assert!(web.logs().contains("took over (FromRails)"), "{}", web.logs());
}

/// r3.1 (R-B5-OVERRIDES): an umbrelOS 2.0 user-settings file binding another host folder, holding a 2.60.0 install, at
/// `web`'s /app/storage. The step reads the app's own `home` (empty): exit 0, silent, nothing created. `web` reads the
/// other folder and refuses it as Behind in Umbrel's words (C1-U, 503); that folder stays byte-equal.
#[test]
fn umbrels_step_leaves_a_storage_override_on_web_to_web() {
    let Some(image) = image() else { return };
    let other = umbrel(&image);
    other.install_2x(UMBREL_2X, RAILS_2_60).ok(&["stop"]);
    let before = snapshot(&image, &other.storage);
    let u = umbrel(&image);
    let Volume::At(mount) = &other.storage else { unreachable!() };
    let settings = u.app(UMBREL, &[("web", &image)], &[&format!("services:\n  web:\n    volumes:\n      - {mount}\n")]);
    up_ok(&settings.compose(&["up", "--detach", "--build"]));
    let web = settings.container("web");
    web.wait_for("/health-check", 503);
    step_silent(&u);
    assert_eq!(storage_source(&u.named("web")), mount.trim_end_matches(":/app/storage"));
    let page = web.exec(&["curl", "-s", "http://localhost:3000/"]);
    assert!(page.contains(&on_page(C1_UMBREL)), "{page}");
    settings.ok(&["stop"]);
    assert_eq!(snapshot(&image, &other.storage), before, "the overriding folder is untouched");
    assert_eq!(snapshot(&image, &u.storage), ".", "the app's own folder stays empty");
}

/// Every value in `api_keys`, `app_configs` and `users` that is stored encrypted (`{"p":…`), by table, row and column.
fn encrypted_values(image: &str, storage: &Volume) -> Vec<(String, i64, String, String)> {
    let copy = take(image, storage);
    let c = rusqlite::Connection::open(copy.path().join("production.sqlite3")).unwrap();
    let mut out = vec![];
    for table in ["api_keys", "app_configs", "users"] {
        let mut statement = c.prepare(&format!("SELECT rowid, * FROM {table}")).unwrap();
        let columns: Vec<String> = statement.column_names().iter().map(|s| s.to_string()).collect();
        let mut rows = statement.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let id: i64 = row.get(0).unwrap();
            for (i, column) in columns.iter().enumerate().skip(1) {
                if let Ok(Some(value)) = row.get::<_, Option<String>>(i) {
                    if value.starts_with("{\"p\":") { out.push((table.to_string(), id, column.clone(), value)); }
                }
            }
        }
    }
    out
}

/// r4.1: a derived-key 2.34.0 install (its `.secrets` without the pair; the keys derive from SECRET_KEY_BASE S1, which
/// umbrelOS 2.0's user settings give `web` alone) with SEED_2_34's ApiKey and an encrypted AppConfig. The step runs
/// 2.70.21's migrate under the shipped `${APP_SEED}-secret-key-base` (not S1): every encrypted value of api_keys,
/// app_configs and users and `.secrets` stay byte-equal. The next start runs the step again (now silent) first. `web` (S1) then takes the install over with no C9 and the
/// ApiKey reads under S1's keys.
#[test]
fn umbrels_step_under_another_secret_changes_no_encrypted_value() {
    let Some(image) = image() else { return };
    let own = "SECRET_KEY_BASE=b5-2-own-secret-of-at-least-thirty-two-characters";
    let u = umbrel(&image);
    u.rails_with(own, RAILS_2_34, &["migrate"]);
    docker(&["run", "--rm", "--network", "none", "--user", "0", "--entrypoint", "rm", "-v", &u.storage.mount(), RAILS_2_34, "/app/storage/.secrets"]);
    let seed = format!("AppConfig.create!(key: 'coingecko_api_key', value: 'b5-2-sample')\n{SEED_2_34}");
    let said = u.rails_with(own, RAILS_2_34, &["rake", "runner", &seed]);
    assert!(said.ends_with(", queue 0"), "{said}");
    let copy = take(&image, &u.storage);
    let secrets = std::fs::read(copy.path().join(".secrets")).unwrap();
    let file = String::from_utf8_lossy(&secrets);
    assert!(!file.contains("ACTIVE_RECORD_ENCRYPTION") && !file.contains("SECRET_KEY_BASE="), "derived keys: {file}");
    let cipher = Cipher::new(&keys_2x(copy.path(), &[own]).1);
    let encrypted = encrypted_values(&image, &u.storage);
    let tables: Vec<&str> = encrypted.iter().map(|(t, ..)| t.as_str()).collect();
    assert!(tables.contains(&"api_keys") && tables.contains(&"app_configs"), "{encrypted:?}");

    step_upgrades(&image, &u);
    assert_eq!(encrypted_values(&image, &u.storage), encrypted, "the step changed no encrypted value");
    assert_eq!(std::fs::read(take(&image, &u.storage).path().join(".secrets")).unwrap(), secrets, "nor .secrets");

    let settings = u.app(UMBREL, &[("web", &image)], &[&format!("services:\n  web:\n    environment:\n      SECRET_KEY_BASE: {}\n", own.trim_start_matches("SECRET_KEY_BASE="))]);
    up_ok(&u.update(&u.project, &settings));
    let web = settings.container("web");
    web.wait_for("/health-check", 200);
    step_silent(&u);
    assert!(web.logs().contains("took over (FromRails)") && !web.logs().contains("can't read this install"), "{}", web.logs());
    settings.ok(&["stop"]);
    left_taken_over(&image, &u.storage, &cipher, &secrets);
    let copy = take(&image, &u.storage);
    let c = rusqlite::Connection::open(copy.path().join("production.sqlite3")).unwrap();
    let key: String = c.query_row("SELECT key FROM api_keys", [], |r| r.get(0)).unwrap();
    assert_eq!(cipher.decrypt(&key).unwrap(), "b3-6-key");
}

/// r5.2 (R-B5-OVERRIDES): an umbrelOS 2.0 user-settings file setting only `web`'s QUEUE_DATABASE_PATH, on a Behind 2.34.0
/// install with SEED_2_34's data. The step migrates and its migrations enqueue their three jobs in the default queue
/// (the step can't see `web`'s settings); 3.0 imports only its own queue, so it takes the install over with none of
/// them, and they stay where the step put them. Nothing fails; the tracker waits for its next sync (PLAN-B5 r5.2).
#[test]
fn umbrels_step_with_a_queue_override_on_web_leaves_its_jobs_in_the_default_queue() {
    let Some(image) = image() else { return };
    let u = umbrel(&image);
    behind_2_34(&u);
    // The install ran 2.x with the same setting, so its queue file is there (empty: SEED_2_34 left nothing queued).
    docker(&["run", "--rm", "--network", "none", "-v", &u.storage.mount(), "-e", UMBREL_SECRET, "-e", "QUEUE_DATABASE_PATH=/app/storage/custom_queue.sqlite3",
             RAILS_2_34, "migrate"]);
    assert!(take(&image, &u.storage).path().join("custom_queue.sqlite3").exists());
    let settings = u.app(UMBREL, &[("web", &image)], &["services:\n  web:\n    environment:\n      QUEUE_DATABASE_PATH: /app/storage/custom_queue.sqlite3\n"]);
    up_ok(&settings.compose(&["up", "--detach", "--build"]));
    let web = settings.container("web");
    web.wait_for("/health-check", 200);
    step_migrated(&image, &u);
    let took = web.logs().lines().find(|l| l.contains("took over (")).unwrap_or_default().to_string();
    eprintln!("{took}");
    assert!(took.contains("took over (FromRails)") && took.contains(" 0 queued job(s) removed"), "{}", web.logs());
    settings.ok(&["stop"]);
    let copy = take(&image, &u.storage);
    let queue = rusqlite::Connection::open(copy.path().join("production_queue.sqlite3")).unwrap();
    let classes: Vec<String> = queue.prepare("SELECT class_name FROM solid_queue_jobs ORDER BY class_name").unwrap()
        .query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(classes, ["AccountTransaction::ResolveAssetsJob", "PortfolioSnapshot::BackfillJob", "Tracker::LedgerJob"]);
}
