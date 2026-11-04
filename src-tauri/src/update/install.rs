//! B7's own replacement step (PLAN-B7 r3.1 as amended by r4.2, r4.4, r5.1, r5.5), and the launcher's side of the app's
//! bundle lock (r4.1, r5.3; the engine's side is rust/src/bundle.rs).
//!
//! Each install checks that the package is this platform's (r5.1), then makes exactly one commit step: macOS one
//! `renamex_np(RENAME_SWAP)` of the installed app and the extracted one; AppImage one `rename` over `$APPIMAGE`; Windows
//! the installer's launch. A failure before the commit leaves the installed app as it was; after it, the update counts as
//! installed. Test builds (any identifier but the released app's) can fail any step on purpose
//! (`DELTABADGER_TEST_INSTALL_FAULT=<step>`).
use std::io;
use std::path::{Path, PathBuf};

/// An install that didn't commit: the installed app is untouched.
#[derive(Debug)]
pub struct Failure {
    pub reason: String,
    /// The disk is full (U5c).
    pub space: bool,
}

impl Failure {
    fn new(reason: impl Into<String>) -> Self { Failure { reason: reason.into(), space: false } }
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Failure { reason: error.to_string(), space: full(&error) }
    }
}

/// A full disk, also under the context tar wraps an extraction error in.
fn full(error: &io::Error) -> bool {
    #[cfg(unix)]
    let code = Some(libc::ENOSPC);
    #[cfg(not(unix))]
    let code = Some(112); // ERROR_DISK_FULL
    error.kind() == io::ErrorKind::StorageFull
        || error.raw_os_error() == code
        || error.get_ref().and_then(|inner| inner.downcast_ref::<io::Error>()).is_some_and(full)
        || error.get_ref().and_then(|inner| inner.source()).and_then(|s| s.downcast_ref::<io::Error>()).is_some_and(full)
}

fn fault(fault: Option<&str>, step: &str) -> io::Result<()> {
    if fault == Some(step) { Err(io::Error::other(format!("test fault at {step}"))) } else { Ok(()) }
}

fn random() -> String {
    let mut bytes = [0u8; 6];
    let _ = getrandom::fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---- The bundle lock (r4.1, r5.3): the same file rust/src/bundle.rs `lock_file` names for this app. ----

/// The installed app the launcher runs from: macOS the `.app` around `Contents/MacOS/<exe>`; AppImage `$APPIMAGE`;
/// Windows the folder of `Deltabadger.exe`. The engine computes the same path from its sidecar (bundle.rs `identity`).
pub fn installed_app(exe: &Path, env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let dir = exe.parent()?;
    if cfg!(target_os = "macos") {
        let contents = dir.parent()?;
        let app = contents.parent()?;
        (dir.file_name()? == "MacOS" && contents.file_name()? == "Contents" && app.extension()? == "app").then(|| app.to_path_buf())
    } else if cfg!(windows) {
        Some(dir.to_path_buf())
    } else {
        let image = env("APPIMAGE")?;
        Some(Path::new(&image).canonicalize().unwrap_or_else(|_| image.into()))
    }
}

/// `bundle-<first 16 hex of sha256(identity)>.lock` (bundle.rs `lock_file`).
pub fn lock_name(identity: &Path) -> String {
    use sha2::Digest;
    let hash = sha2::Sha256::digest(identity.to_string_lossy().as_bytes());
    format!("bundle-{}.lock", hash.iter().take(8).map(|b| format!("{b:02x}")).collect::<String>())
}

/// One folder per machine (bundle.rs `lock_dir`).
pub fn lock_dir(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some("/Users/Shared/.deltabadger".into())
    } else if cfg!(windows) {
        env("LOCALAPPDATA").map(|d| Path::new(&d).join("Deltabadger"))
    } else {
        Some("/tmp/.deltabadger".into())
    }
}

/// Opens the lock file by the engine's protocol (bundle.rs `open`: created for every account, modes set by their creator
/// only, symlinks refused) and requires it healthy before an install trusts it (r5.3): the folder 1777, the file a
/// regular 0666 file opened read-write. Else the error: no in-app install on this machine (U14).
pub fn open_lock(path: &Path) -> Result<std::fs::File, String> {
    let dir = path.parent().ok_or("the lock has no folder")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
        let fail = |what: &str, e: io::Error| format!("{what} {}: {e}", path.display());
        match std::fs::DirBuilder::new().mode(0o1777).create(dir) {
            Ok(()) => std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o1777)).map_err(|e| fail("chmod the folder of", e))?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(fail("create the folder of", e)),
        }
        let folder = std::fs::symlink_metadata(dir).map_err(|e| fail("read the folder of", e))?;
        if !folder.file_type().is_dir() || folder.permissions().mode() & 0o7777 != 0o1777 {
            return Err(format!("{} is not a folder with mode 1777", dir.display()));
        }
        let open = |create: bool| {
            let mut options = std::fs::OpenOptions::new();
            options.read(true).write(true).custom_flags(libc::O_NOFOLLOW);
            if create {
                options.create_new(true).mode(0o666);
            }
            options.open(path)
        };
        let file = match open(true) {
            Ok(f) => {
                f.set_permissions(std::fs::Permissions::from_mode(0o666)).map_err(|e| fail("chmod", e))?;
                f
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => open(false).map_err(|e| fail("open", e))?,
            Err(e) => return Err(fail("create", e)),
        };
        let meta = file.metadata().map_err(|e| fail("read", e))?;
        if !meta.is_file() || meta.permissions().mode() & 0o777 != 0o666 {
            return Err(format!("{} is not a regular file with mode 0666", path.display()));
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path).map_err(|e| format!("open {}: {e}", path.display()))
    }
}

/// The lock taken exclusively: Ok(None) while any engine of this app holds its share.
pub fn try_exclusive(file: std::fs::File) -> Result<Option<std::fs::File>, String> {
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e.to_string()),
    }
}

// ---- macOS (r3.1, r4.4, r5.1, r5.5) ----

/// The package is a gzip tar with exactly one top-level `*.app` whose launcher and engine are arm64 Mach-O (r5.1).
/// Returns the `.app`'s name.
pub fn check_macos(bytes: &[u8]) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        use std::io::Read;
        use std::path::Component;
        let mut tops = std::collections::BTreeSet::new();
        let (mut launcher, mut engine) = (false, false);
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
        for entry in archive.entries().map_err(|e| format!("not a gzip tar: {e}"))? {
            let mut entry = entry.map_err(|e| format!("not a gzip tar: {e}"))?;
            let path = entry.path().map_err(|e| e.to_string())?.into_owned();
            let mut parts = Vec::new();
            for part in path.components() {
                match part {
                    Component::Normal(p) => parts.push(p.to_string_lossy().into_owned()),
                    Component::CurDir => {}
                    _ => return Err(format!("unexpected path in the package: {}", path.display())),
                }
            }
            let Some(top) = parts.first() else { continue };
            tops.insert(top.clone());
            let inner = parts[1..].join("/");
            if inner == "Contents/MacOS/Deltabadger" || inner == "Contents/MacOS/deltabadger-engine" {
                let mut head = [0u8; 8];
                entry.read_exact(&mut head).map_err(|_| format!("{inner} is too short"))?;
                if !arm64_macho(&head) {
                    return Err(format!("{inner} is not an arm64 Mach-O"));
                }
                if inner.ends_with("Deltabadger") { launcher = true } else { engine = true }
            }
        }
        let top = match tops.len() {
            1 => tops.into_iter().next().unwrap_or_default(),
            n => return Err(format!("{n} top-level entries in the package, not one app")),
        };
        if !top.ends_with(".app") || !launcher || !engine {
            return Err(format!("{top} is not an app with Deltabadger and deltabadger-engine"));
        }
        Ok(top)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = bytes;
        Err("not a macOS app".into())
    }
}

/// `MH_MAGIC_64`, `CPU_TYPE_ARM64`.
pub fn arm64_macho(head: &[u8]) -> bool {
    head.len() >= 8 && head[..4] == [0xcf, 0xfa, 0xed, 0xfe] && head[4..8] == 0x0100_000c_u32.to_le_bytes()
}

#[cfg(target_os = "macos")]
fn swap(a: &Path, b: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let a = std::ffi::CString::new(a.as_os_str().as_bytes())?;
    let b = std::ffi::CString::new(b.as_os_str().as_bytes())?;
    // SAFETY: two NUL-terminated paths that live across the call.
    if unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) } == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

/// Before a download (r4.4): two empty folders next to the app, swapped and removed. Fails where the folder isn't
/// writable or the volume can't swap (HFS+): the app can't replace itself there.
#[cfg(target_os = "macos")]
pub fn swap_probe(app: &Path) -> io::Result<()> {
    let parent = app.parent().ok_or_else(|| io::Error::other("the app has no folder"))?;
    let id = random();
    let (a, b) = (parent.join(format!(".deltabadger-probe-{id}-a")), parent.join(format!(".deltabadger-probe-{id}-b")));
    std::fs::create_dir(&a)?;
    let result = std::fs::create_dir(&b).and_then(|()| swap(&a, &b));
    let _ = std::fs::remove_dir(&a);
    let _ = std::fs::remove_dir(&b);
    result
}

/// The prefix of the folder an install extracts into, next to the app.
fn temp_prefix(app: &Path) -> String {
    format!(".{}.update-", app.file_name().map_or_else(|| "app".into(), |n| n.to_string_lossy().into_owned()))
}

/// What an install left beside the app after its commit (the old app, when deleting it failed, r5.5): removed at start.
pub fn remove_leftovers(app: &Path, log: &dyn Fn(&str)) {
    let (Some(parent), prefix) = (app.parent(), temp_prefix(app)) else { return };
    let Ok(entries) = std::fs::read_dir(parent) else { return };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            match std::fs::remove_dir_all(entry.path()) {
                Ok(()) => log(&format!("desktop: update: removed {}", entry.path().display())),
                Err(e) => log(&format!("desktop: update: could not remove {}: {e}", entry.path().display())),
            }
        }
    }
}

/// macOS: extract next to the app, mode 0755, one swap (the commit), then the old app deleted (non-fatal).
#[cfg(target_os = "macos")]
pub fn install_macos(app: &Path, bytes: &[u8], test_fault: Option<&str>, log: &dyn Fn(&str)) -> Result<(), Failure> {
    use std::os::unix::fs::PermissionsExt;
    fault(test_fault, "format")?;
    let name = check_macos(bytes).map_err(Failure::new)?;
    let parent = app.parent().ok_or_else(|| Failure::new("the app has no folder"))?;
    let temp = parent.join(format!("{}{}", temp_prefix(app), random()));
    let before = || -> Result<(), Failure> {
        std::fs::create_dir(&temp)?;
        fault(test_fault, "extract")?;
        tar::Archive::new(flate2::read::GzDecoder::new(bytes)).unpack(&temp)?;
        fault(test_fault, "mode")?;
        std::fs::set_permissions(temp.join(&name), std::fs::Permissions::from_mode(0o755))?;
        fault(test_fault, "swap")?;
        swap(app, &temp.join(&name))?;
        Ok(())
    };
    if let Err(failure) = before() {
        let _ = std::fs::remove_dir_all(&temp);
        return Err(failure);
    }
    // Committed: the new app is in place. The old one, now in `temp`, goes; if it can't, the next start removes it.
    match fault(test_fault, "after-swap").and_then(|()| std::fs::remove_dir_all(&temp)) {
        Ok(()) => {}
        Err(e) => log(&format!("desktop: update: the previous app stays in {} until the next start: {e}", temp.display())),
    }
    Ok(())
}

// ---- AppImage (r3.1, r4.2), compiled and tested here, proved in B7-2 ----

/// ELF, 64-bit, little-endian, x86-64 (r5.1).
pub fn check_appimage(bytes: &[u8]) -> Result<(), String> {
    let ok = bytes.len() >= 20 && bytes[..4] == [0x7f, b'E', b'L', b'F'] && bytes[4] == 2 && bytes[5] == 1 && bytes[18..20] == [0x3e, 0x00];
    if ok { Ok(()) } else { Err("the package is not an x86-64 AppImage".into()) }
}

/// AppImage: a temp file in the same folder, fsynced, the old file's mode, one rename over it (the commit), the folder
/// fsynced after (logged on failure, r4.2).
#[cfg(unix)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn install_appimage(target: &Path, bytes: &[u8], test_fault: Option<&str>, log: &dyn Fn(&str)) -> Result<(), Failure> {
    use std::io::Write;
    fault(test_fault, "format")?;
    check_appimage(bytes).map_err(Failure::new)?;
    let dir = target.parent().ok_or_else(|| Failure::new("the AppImage has no folder"))?;
    let temp = dir.join(format!("{}{}", temp_prefix(target), random()));
    let before = || -> Result<(), Failure> {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
        fault(test_fault, "write")?;
        file.write_all(bytes)?;
        fault(test_fault, "fsync")?;
        file.sync_all()?;
        fault(test_fault, "mode")?;
        std::fs::set_permissions(&temp, std::fs::metadata(target)?.permissions())?;
        fault(test_fault, "rename")?;
        std::fs::rename(&temp, target)?;
        Ok(())
    };
    if let Err(failure) = before() {
        let _ = std::fs::remove_file(&temp);
        return Err(failure);
    }
    if let Err(e) = fault(test_fault, "after-rename").and_then(|()| std::fs::File::open(dir)?.sync_all()) {
        log(&format!("desktop: update: the AppImage's folder wasn't synced: {e}"));
    }
    Ok(())
}

// ---- Windows (r3.1, r4.3), the parts compiled and tested here; the installer launch is proved in B7-2 ----

/// `MZ`, then `PE\0\0` with machine x86-64 (r5.1).
pub fn check_windows(bytes: &[u8]) -> Result<(), String> {
    let pe = bytes.get(0x3c..0x40).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let ok = bytes.starts_with(b"MZ")
        && pe.is_some_and(|pe| bytes.get(pe..pe + 4) == Some(b"PE\0\0") && bytes.get(pe + 4..pe + 6) == Some(&[0x64, 0x86]));
    if ok { Ok(()) } else { Err("the package is not an x86-64 Windows installer".into()) }
}

/// The installer's arguments (the plugin's, updater.rs): passive update; `/R` relaunches (click, not Quit).
pub fn installer_args(relaunch: bool) -> &'static str { if relaunch { "/P /UPDATE /R" } else { "/P /UPDATE" } }

/// `<data>/updates/attempt` = {version, intent}, fsynced before the handoff (r4.3): the next start reads the outcome
/// from the version that runs.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn write_attempt(data: &Path, version: &str, relaunch: bool) -> io::Result<()> {
    use std::io::Write;
    let dir = data.join("updates");
    std::fs::create_dir_all(&dir)?;
    let body = serde_json::json!({ "version": version, "intent": if relaunch { "click" } else { "quit" } });
    let mut file = std::fs::File::create(dir.join("attempt"))?;
    file.write_all(body.to_string().as_bytes())?;
    file.sync_all()
}

/// Windows: the installer written to a new temp folder, the attempt mark, then its launch. The caller exits the app only
/// once this returned Ok.
#[cfg(windows)]
pub fn launch_windows_installer(bytes: &[u8], data: &Path, version: &str, relaunch: bool, test_fault: Option<&str>) -> Result<(), Failure> {
    use std::io::Write;
    use std::os::windows::ffi::OsStrExt;
    fault(test_fault, "format")?;
    check_windows(bytes).map_err(Failure::new)?;
    let dir = std::env::temp_dir().join(format!("deltabadger-update-{}", random()));
    std::fs::create_dir(&dir)?;
    let setup = dir.join("setup.exe");
    let mut file = std::fs::File::create(&setup)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    write_attempt(data, version, relaunch)?;
    fault(test_fault, "launch")?;
    let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
    let (file, args, verb) = (wide(setup.as_os_str()), wide(installer_args(relaunch).as_ref()), wide("open".as_ref()));
    // SAFETY: NUL-terminated UTF-16 strings that live across the call.
    let result = unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            args.as_ptr(),
            std::ptr::null(),
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW,
        )
    };
    // ShellExecuteW reports success as a value above 32.
    if result as usize > 32 { Ok(()) } else { Err(Failure::new(format!("the installer didn't start ({})", result as usize))) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("b7-install-{label}-{}", random()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The same vector as rust/src/bundle.rs's test: the launcher and the engine name one file for one app.
    #[test]
    fn the_lock_name_matches_the_engines() {
        assert_eq!(lock_name(Path::new("/Applications/Deltabadger.app")), "bundle-38dd6f0c052bec9f.lock");
    }

    #[test]
    fn the_installed_app_is_the_bundle_around_the_launcher() {
        if cfg!(target_os = "macos") {
            let exe = Path::new("/Applications/Deltabadger.app/Contents/MacOS/Deltabadger");
            assert_eq!(installed_app(exe, &|_| None), Some(PathBuf::from("/Applications/Deltabadger.app")));
            assert_eq!(installed_app(Path::new("/usr/local/bin/Deltabadger"), &|_| None), None);
        }
    }

    #[test]
    fn formats_are_checked_by_their_headers() {
        assert!(arm64_macho(&[0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0x00, 0x00, 0x01]));
        assert!(!arm64_macho(&[0xcf, 0xfa, 0xed, 0xfe, 0x07, 0x00, 0x00, 0x01])); // x86-64
        let mut elf = vec![0u8; 64];
        elf[..6].copy_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1]);
        elf[18] = 0x3e;
        assert!(check_appimage(&elf).is_ok());
        elf[18] = 0xb7; // aarch64
        assert!(check_appimage(&elf).is_err());
        let mut pe = vec![0u8; 0x100];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3c] = 0x80;
        pe[0x80..0x86].copy_from_slice(b"PE\0\0\x64\x86");
        assert!(check_windows(&pe).is_ok());
        pe[0x84..0x86].copy_from_slice(&[0x64, 0xaa]); // arm64
        assert!(check_windows(&pe).is_err());
        assert!(check_windows(&elf).is_err() && check_appimage(&pe).is_err());
        assert!(check_macos(&elf).is_err());
    }

    #[test]
    fn a_full_disk_is_found_under_the_extraction_context() {
        assert!(Failure::from(io::Error::new(io::ErrorKind::StorageFull, "unpacking")).space);
        #[cfg(unix)]
        {
            let enospc = io::Error::from_raw_os_error(libc::ENOSPC);
            assert!(Failure::from(io::Error::other(enospc)).space, "wrapped");
        }
        assert!(!Failure::from(io::Error::new(io::ErrorKind::PermissionDenied, "x")).space);
    }

    #[test]
    fn the_installer_relaunches_only_after_a_click_and_the_attempt_is_durable() {
        assert_eq!(installer_args(true), "/P /UPDATE /R");
        assert_eq!(installer_args(false), "/P /UPDATE");
        let data = temp("attempt");
        write_attempt(&data, "9.9.9", false).unwrap();
        let written: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(data.join("updates/attempt")).unwrap()).unwrap();
        assert_eq!(written, serde_json::json!({"version": "9.9.9", "intent": "quit"}));
        let _ = std::fs::remove_dir_all(data);
    }

    #[cfg(unix)]
    #[test]
    fn an_appimage_is_replaced_by_one_rename_and_left_alone_by_any_failure_before_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("appimage");
        let target = dir.join("Deltabadger.AppImage");
        std::fs::write(&target, b"old").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o751)).unwrap();
        let mut new = vec![0u8; 64];
        new[..6].copy_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1]);
        new[18] = 0x3e;
        for step in ["format", "write", "fsync", "mode", "rename"] {
            assert!(install_appimage(&target, &new, Some(step), &|_| {}).is_err(), "{step}");
            assert_eq!(std::fs::read(&target).unwrap(), b"old", "{step}");
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "{step}: a temp file left");
        }
        assert!(install_appimage(&target, b"#!/bin/sh", None, &|_| {}).is_err(), "another format");
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        let logged = std::cell::RefCell::new(Vec::new());
        install_appimage(&target, &new, Some("after-rename"), &|l| logged.borrow_mut().push(l.to_string())).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), new, "after the commit a later failure is only logged");
        assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o751);
        assert_eq!(logged.borrow().len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(target_os = "macos")]
    fn package(launcher: [u8; 8], engine: Option<[u8; 8]>) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        let mut add = |path: &str, data: &[u8]| {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, path, data).unwrap();
        };
        add("Deltabadger.app/Contents/MacOS/Deltabadger", &launcher);
        if let Some(engine) = engine {
            add("Deltabadger.app/Contents/MacOS/deltabadger-engine", &engine);
        }
        add("Deltabadger.app/Contents/Info.plist", b"new");
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[cfg(target_os = "macos")]
    const ARM64: [u8; 8] = [0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0x00, 0x00, 0x01];

    #[cfg(target_os = "macos")]
    #[test]
    fn a_macos_package_needs_one_app_with_both_arm64_executables() {
        assert_eq!(check_macos(&package(ARM64, Some(ARM64))).unwrap(), "Deltabadger.app");
        assert!(check_macos(&package(ARM64, None)).is_err());
        let x86 = [0xcf, 0xfa, 0xed, 0xfe, 0x07, 0x00, 0x00, 0x01];
        assert!(check_macos(&package(ARM64, Some(x86))).is_err());
        assert!(check_macos(b"not a tar").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_macos_install_swaps_once_and_any_failure_before_the_swap_leaves_the_app() {
        let dir = temp("macos");
        let app = dir.join("Deltabadger.app");
        std::fs::create_dir_all(app.join("Contents")).unwrap();
        std::fs::write(app.join("Contents/Info.plist"), b"old").unwrap();
        let bytes = package(ARM64, Some(ARM64));
        swap_probe(&app).unwrap();
        for step in ["format", "extract", "mode", "swap"] {
            assert!(install_macos(&app, &bytes, Some(step), &|_| {}).is_err(), "{step}");
            assert_eq!(std::fs::read(app.join("Contents/Info.plist")).unwrap(), b"old", "{step}");
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "{step}: a temp folder left");
        }
        install_macos(&app, &bytes, Some("after-swap"), &|_| {}).unwrap();
        assert_eq!(std::fs::read(app.join("Contents/Info.plist")).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2, "the old app is left after the deletion fault");
        remove_leftovers(&app, &|_| {});
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "removed at the next start");
        install_macos(&app, &bytes, None, &|_| {}).unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_probe_refuses_a_folder_the_app_cannot_swap_in() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("probe");
        let app = dir.join("Deltabadger.app");
        std::fs::create_dir(&app).unwrap();
        swap_probe(&app).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        assert!(swap_probe(&app).is_err(), "a read-only folder");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "the probe leaves nothing");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn an_unhealthy_lock_is_never_trusted() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("lock").join("shared");
        let path = dir.join(lock_name(Path::new("/x.app")));
        let held = open_lock(&path).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o7777, 0o1777);
        held.lock_shared().unwrap();
        assert!(try_exclusive(open_lock(&path).unwrap()).unwrap().is_none(), "an engine holds its share");
        drop(held);
        assert!(try_exclusive(open_lock(&path).unwrap()).unwrap().is_some());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(open_lock(&path).is_err(), "a file left 0600");
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", &path).unwrap();
        assert!(open_lock(&path).is_err(), "a symlink");
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
    }
}
