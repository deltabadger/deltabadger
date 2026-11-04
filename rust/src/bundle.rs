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
    // Only the app's engine, by its sidecar name (PLAN-B6 §2): on Windows the engine built on its own is
    // `deltabadger.exe`, which a case-insensitive file system would take for the app's `Deltabadger.exe`.
    if !exe.file_stem().is_some_and(|n| n.eq_ignore_ascii_case("deltabadger-engine")) { return None }
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
/// mode (nobody can chmod another account's file). Symlinks are refused without being followed — the folder (checked
/// before use) and the file (`O_NOFOLLOW`, then the open descriptor itself must be a regular file) — so no account can
/// point another's engine at a different file than the updater locks.
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
        if !std::fs::symlink_metadata(dir)?.file_type().is_dir() { return Err(Error::other("the lock folder is not a folder")) }
        let opened = match std::fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o666).custom_flags(libc::O_NOFOLLOW).open(path) {
            Ok(f) => { f.set_permissions(std::fs::Permissions::from_mode(0o666))?; f }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NOFOLLOW).open(path)?,
            Err(e) => return Err(e),
        };
        if !opened.metadata()?.is_file() { return Err(Error::other("the lock is not a regular file")) }
        Ok(opened)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)?;
        std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)
    }
}

/// The installed app's file or folder as it is now: replacing the app (the macOS swap, the AppImage rename) gives the
/// path another one (PLAN-B7 r3.1).
#[cfg(unix)]
fn generation(app: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(app).ok().map(|m| (m.dev(), m.ino()))
}
#[cfg(not(unix))]
fn generation(_: &Path) -> Option<(u64, u64)> { None }

/// `main`'s first act. Inside an installed app: refuses while the app is being replaced (Windows: the installer's
/// `.updating` mark, PLAN-B7 r3.3; everywhere: the lock held exclusively), else returns the held shared lock. A lock file
/// that can't be used is logged and the engine runs without it: a broken lock never stops bots; the updater refuses to
/// install in place while the lock isn't healthy (r5.3).
pub fn enter(env: &dyn Fn(&str) -> Option<String>) -> Result<Option<File>, &'static str> {
    let Some(app) = std::env::current_exe().and_then(|e| e.canonicalize()).ok().and_then(|exe| identity(&exe, env)) else { return Ok(None) };
    if cfg!(windows) && app.join(".updating").exists() { return Err(BEING_UPDATED) }
    // Before any wait: the app this process was started from.
    let started_from = generation(&app);
    let Some(path) = lock_file(&app, env) else { return Ok(None) };
    let file = match open(&path) {
        Ok(f) => f,
        Err(e) => { crate::eline!("deltabadger: can't use the update lock {}: {e}; running without it", path.display()); return Ok(None) }
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match file.try_lock_shared() {
            // An engine that waited out an update would run the replaced app's code beside the new app: it stops too.
            Ok(()) if generation(&app) != started_from => return Err(BEING_UPDATED),
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

#[cfg(test)]
mod tests {
    /// The same vector as src-tauri/src/update/install.rs's test: the engine and the launcher name one file for one app.
    #[test]
    fn the_lock_name_matches_the_launchers() {
        let env = |k: &str| (k == "LOCALAPPDATA").then(|| "C:\\Users\\x\\AppData\\Local".to_string());
        let file = super::lock_file(std::path::Path::new("/Applications/Deltabadger.app"), &env).unwrap();
        assert_eq!(file.file_name().unwrap(), "bundle-38dd6f0c052bec9f.lock");
    }
}
