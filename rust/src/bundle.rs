//! The installed desktop app an engine runs from, and its update lock (PLAN-B7 r3.2, r4.1, r5.3). Every engine process
//! of an installed app holds a shared lock on one file per app for as long as it runs; the app's updater takes the same
//! file exclusively before it replaces the app, so it never replaces an app any engine still runs from — a refusal page, a
//! command in a terminal, another account's engine. An engine that can't get its share within 2 s exits: the app is being
//! updated. Outside an installed app (Docker, a .deb, a build tree) there is no lock: nothing replaces those in place.
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Why an engine of an installed app doesn't start: the app is being replaced.
pub const BEING_UPDATED: &str = "this copy of Deltabadger is being updated; open it again in a moment";

/// The installed app `exe` belongs to: on macOS the `.app` whose `Contents/MacOS` holds it; in an AppImage the AppImage
/// file (`APPIMAGE`), when `exe` runs from its mount (`APPDIR`); on Windows the folder that also holds `Deltabadger.exe`.
pub fn identity(exe: &Path, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let dir = exe.parent()?;
    if cfg!(target_os = "macos") {
        let contents = dir.parent()?;
        let app = contents.parent()?;
        (dir.file_name()? == "MacOS" && contents.file_name()? == "Contents" && app.extension()? == "app").then(|| app.to_path_buf())
    } else if cfg!(windows) {
        dir.join("Deltabadger.exe").is_file().then(|| dir.to_path_buf())
    } else {
        let (image, mount) = (env("APPIMAGE")?, env("APPDIR")?);
        let mount = Path::new(&mount).canonicalize().ok()?;
        exe.starts_with(&mount).then(|| Path::new(&image).canonicalize().unwrap_or_else(|_| image.into()))
    }
}

/// One place per machine, whatever the environment: every account's engine of one app finds the same file.
fn lock_dir(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    if let Some(dir) = env("DELTABADGER_TEST_BUNDLE_LOCK_DIR") { return Some(dir.into()) }
    if cfg!(target_os = "macos") { Some("/Users/Shared/.deltabadger".into()) }
    // NSIS installs are per account: so is the lock.
    else if cfg!(windows) { env("LOCALAPPDATA").map(|d| Path::new(&d).join("Deltabadger")) }
    else { Some("/tmp/.deltabadger".into()) }
}

/// The lock file of the app at `identity`.
pub fn lock_file(identity: &Path, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    use sha2::Digest;
    let hash = hex::encode(sha2::Sha256::digest(identity.to_string_lossy().as_bytes()));
    Some(lock_dir(env)?.join(format!("bundle-{}.lock", &hash[..16])))
}

/// Opens the lock file, creating it and its folder for every account if they don't exist. Only their creator sets their
/// mode (nobody can chmod another account's file); a symlink or anything but a regular file is refused.
pub fn open(path: &Path) -> std::io::Result<File> {
    use std::io::{Error, ErrorKind};
    let dir = path.parent().ok_or_else(|| Error::new(ErrorKind::InvalidInput, "no folder"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        match std::fs::DirBuilder::new().mode(0o1777).create(dir) {
            Ok(()) => std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o1777))?,
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        if std::fs::symlink_metadata(path).is_ok_and(|m| !m.file_type().is_file()) { return Err(Error::other("not a regular file")) }
        match std::fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o666).open(path) {
            Ok(f) => { f.set_permissions(std::fs::Permissions::from_mode(0o666))?; Ok(f) }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => std::fs::OpenOptions::new().read(true).write(true).open(path),
            Err(e) => Err(e),
        }
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)?;
        std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)
    }
}

/// `main`'s first act. Inside an installed app: refuses while the app is being replaced (Windows: the installer's
/// `.updating` mark, PLAN-B7 r3.3; everywhere: the lock held exclusively), else returns the held shared lock. A lock file
/// that can't be used is logged and the engine runs without it: a broken lock never stops bots; the updater refuses to
/// install in place while the lock isn't healthy (r5.3).
pub fn enter(env: &dyn Fn(&str) -> Option<String>) -> Result<Option<File>, &'static str> {
    let Some(app) = std::env::current_exe().and_then(|e| e.canonicalize()).ok().and_then(|exe| identity(&exe, env)) else { return Ok(None) };
    if cfg!(windows) && app.join(".updating").exists() { return Err(BEING_UPDATED) }
    let Some(path) = lock_file(&app, env) else { return Ok(None) };
    let file = match open(&path) {
        Ok(f) => f,
        Err(e) => { crate::eline!("deltabadger: can't use the update lock {}: {e}; running without it", path.display()); return Ok(None) }
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match file.try_lock_shared() {
            Ok(()) => return Ok(Some(file)),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Err(std::fs::TryLockError::WouldBlock) => return Err(BEING_UPDATED),
            Err(std::fs::TryLockError::Error(e)) => {
                crate::eline!("deltabadger: can't use the update lock {}: {e}; running without it", path.display());
                return Ok(None)
            }
        }
    }
}
