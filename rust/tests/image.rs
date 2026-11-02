//! The 3.0 image (Dockerfile.rust), run as Docker runs it (PLAN-B3 §8). Every test needs `DELTABADGER_IMAGE` (an image
//! built from Dockerfile.rust) and a Docker daemon; without the variable they pass without doing anything, so the
//! ordinary test runs never need Docker. CI's `image` job builds the image and sets it.
#![cfg(unix)]

use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn image() -> Option<String> {
    let image = std::env::var("DELTABADGER_IMAGE").ok().filter(|v| !v.trim().is_empty());
    if image.is_none() { eprintln!("DELTABADGER_IMAGE is not set: skipped"); }
    image
}

fn docker(args: &[&str]) -> String {
    let out = Command::new("docker").args(args).output().expect("docker runs");
    assert!(out.status.success(), "docker {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn id(flag: &str) -> String { String::from_utf8(Command::new("id").arg(flag).output().unwrap().stdout).unwrap().trim().to_string() }

/// What /app/storage is. A named volume, as the compose files use (`storage:/app/storage`): a Linux filesystem on every
/// host, so ownership reads as it does in production (Docker Desktop and OrbStack show bind-mounted host files as
/// root's). Or a temp dir bind-mounted there, for a container that runs as this user; dropping it gives its files back
/// to this user first, so the temp dir can be removed.
enum Volume { Named(String), Bind { dir: tempfile::TempDir, image: String } }

impl Volume {
    fn named() -> Self { Volume::Named(docker(&["volume", "create"])) }
    fn bind(image: &str) -> Self { Volume::Bind { dir: tempfile::tempdir().unwrap(), image: image.into() } }
    fn mount(&self) -> String {
        match self { Volume::Named(name) => format!("{name}:/app/storage"), Volume::Bind { dir, .. } => format!("{}:/app/storage", dir.path().display()) }
    }
    fn path(&self) -> &Path { match self { Volume::Bind { dir, .. } => dir.path(), Volume::Named(_) => panic!("a named volume has no host path") } }
}

impl Drop for Volume {
    fn drop(&mut self) {
        match &*self {
            Volume::Named(name) => { let _ = Command::new("docker").args(["volume", "rm", "-f", name]).output(); }
            Volume::Bind { image, .. } => {
                let owner = format!("{}:{}", id("-u"), id("-g"));
                let _ = Command::new("docker").args(["run", "--rm", "--user", "0", "--entrypoint", "chown", "-v", &self.mount(), image, "-R", &owner, "/app/storage"]).output();
            }
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

    /// The HTTP status of GET `path`, or None while nothing answers.
    fn status(&self, path: &str) -> Option<u16> {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port)).ok()?;
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        write!(stream, "GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n").ok()?;
        let mut head = [0u8; 12];
        stream.read_exact(&mut head).ok()?;
        std::str::from_utf8(&head[9..12]).ok()?.parse().ok()
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
