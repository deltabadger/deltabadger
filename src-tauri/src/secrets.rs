//! `.secrets`, the desktop's own key file (PLAN-B6 §1.4): its format, precedence and minting are the 2.x desktop's,
//! and the resolved values reach the engine in its environment. Two inherited defects are fixed (r2.1):
//! D-DESKTOP-SECRETS-MODE (the file is private from its first byte) and D-DESKTOP-SECRETS-DURABLE (its directory entry
//! is synced before any engine starts). `make_durable` completes the same sync before every start, whoever wrote the
//! file (r3.3), so a power loss can never leave an install committed without the keys that read it. On Windows the
//! file's data is flushed and the entry is left to NTFS's journal (r3.2).
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

pub const SECRET_KEY_BASE: &str = "SECRET_KEY_BASE";
pub const AR_PRIMARY_KEY: &str = "ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY";
pub const AR_DERIVATION_SALT: &str = "ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT";
pub const AR_EXTERNAL_MARKER: &str = "ACTIVE_RECORD_ENCRYPTION_KEYS_EXTERNAL";

pub fn nonblank_environment_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

pub fn random_hex(byte_count: usize) -> Result<String, String> {
    let mut bytes = vec![0_u8; byte_count];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("Failed to generate secure random bytes: {error}"))?;

    let mut encoded = String::with_capacity(byte_count * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(encoded)
}

#[cfg(unix)]
fn set_secret_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("Failed to chmod {:?}: {error}", path))
}

#[cfg(not(unix))]
fn set_secret_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn install_may_have_database(database_path: &Path) -> bool {
    nonblank_environment_value("DATABASE_URL").is_some()
        || nonblank_environment_value("PRIMARY_DATABASE_URL").is_some()
        || database_path.exists()
}

/// A new file nobody else can read, from the moment it exists (unix: mode 0600 at create).
fn create_private(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)
}

/// Syncs a directory, so the entries made in it survive a power loss. Unix only: Windows leaves directory entries to
/// the file system's journal (PLAN-B6 r3.2).
fn sync_dir(dir: &Path) -> Result<(), String> {
    hooks::call(hooks::Stage::SyncDir, dir).map_err(|error| format!("Failed to sync {:?}: {error}", dir))?;
    #[cfg(unix)]
    {
        File::open(dir)
            .and_then(|d| d.sync_all())
            .map_err(|error| format!("Failed to sync {:?}: {error}", dir))?;
        hooks::synced(dir);
    }
    Ok(())
}

fn write_secrets_atomically(path: &Path, contents: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Secrets path has no parent: {:?}", path))?;
    let mut temporary_path = None;

    for attempt in 0..100_u32 {
        let candidate = parent.join(format!(".secrets.{}.{}", std::process::id(), attempt));
        match create_private(&candidate) {
            Ok(mut file) => {
                let written = hooks::call(hooks::Stage::Created, &candidate)
                    .and_then(|_| file.write_all(contents.as_bytes()))
                    .and_then(|_| file.sync_all());
                if let Err(error) = written {
                    drop(file);
                    let _ = fs::remove_file(&candidate);
                    return Err(format!("Failed to write {:?}: {error}", candidate));
                }
                temporary_path = Some(candidate);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Failed to create a temporary secrets file: {error}"
                ))
            }
        }
    }

    let temporary_path = temporary_path
        .ok_or_else(|| "Failed to allocate a temporary secrets filename".to_string())?;
    let link_result = fs::hard_link(&temporary_path, path);
    let _ = fs::remove_file(&temporary_path);

    match link_result {
        Ok(()) => log::info!("Generated secrets at {:?}", path),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            log::info!("Another process created {:?}; adopting its secrets", path)
        }
        Err(error) => {
            return Err(format!(
                "Failed to install secrets file {:?}: {error}",
                path
            ))
        }
    }
    hooks::call(hooks::Stage::Linked, path).map_err(|error| format!("Failed to install secrets file {:?}: {error}", path))?;
    sync_dir(parent)
}

fn read_secrets(path: &Path) -> Result<HashMap<String, String>, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read secrets file {:?}: {error}", path))?;
    let mut secrets = HashMap::new();

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            secrets.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    Ok(secrets)
}

/// The keys the engine gets: the environment's where it supplies them, else the file's; on a data dir without a
/// file, a new file first (a key base always, the encryption pair only where no database can exist yet).
pub fn setup_secrets(
    app_data_dir: &Path,
    database_path: &Path,
) -> Result<HashMap<String, String>, String> {
    let secrets_path = app_data_dir.join(".secrets");
    let external_secret = nonblank_environment_value(SECRET_KEY_BASE);
    let external_primary = nonblank_environment_value(AR_PRIMARY_KEY);
    let external_salt = nonblank_environment_value(AR_DERIVATION_SALT);
    let encryption_pair_supplied = external_primary.is_some() || external_salt.is_some();

    if !secrets_path.exists() {
        let mut contents = String::from(
            "# Auto-generated secrets for Deltabadger\n# DO NOT DELETE - required to read encrypted data\n",
        );
        if external_secret.is_none() {
            contents.push_str(&format!("{SECRET_KEY_BASE}={}\n", random_hex(64)?));
        }

        if encryption_pair_supplied {
            contents.push_str(&format!("{AR_EXTERNAL_MARKER}=true\n"));
        } else if !install_may_have_database(database_path) {
            contents.push_str(&format!("{AR_PRIMARY_KEY}={}\n", random_hex(32)?));
            contents.push_str(&format!("{AR_DERIVATION_SALT}={}\n", random_hex(32)?));
        }

        write_secrets_atomically(&secrets_path, &contents)?;
    } else {
        log::info!("Loading existing secrets from {:?}", secrets_path);
    }
    set_secret_permissions(&secrets_path)?;

    let stored = read_secrets(&secrets_path)?;
    let mut resolved = HashMap::new();
    let secret = external_secret
        .or_else(|| {
            stored
                .get(SECRET_KEY_BASE)
                .filter(|v| !v.trim().is_empty())
                .cloned()
        })
        .ok_or_else(|| {
            format!(
                "{SECRET_KEY_BASE} is not supplied and is absent from {:?}",
                secrets_path
            )
        })?;
    resolved.insert(SECRET_KEY_BASE.to_string(), secret);

    if encryption_pair_supplied {
        if let Some(value) = external_primary {
            resolved.insert(AR_PRIMARY_KEY.to_string(), value);
        }
        if let Some(value) = external_salt {
            resolved.insert(AR_DERIVATION_SALT.to_string(), value);
        }
    } else {
        for key in [AR_PRIMARY_KEY, AR_DERIVATION_SALT] {
            if let Some(value) = stored.get(key).filter(|value| !value.trim().is_empty()) {
                resolved.insert(key.to_string(), value.clone());
            }
        }
    }

    if let Some(value) = stored.get(AR_EXTERNAL_MARKER) {
        resolved.insert(AR_EXTERNAL_MARKER.to_string(), value.clone());
    }
    // Old desktop secrets may contain this migration-only key. Preserve it, but never mint it.
    if nonblank_environment_value("APP_ENCRYPTION_KEY").is_none() {
        if let Some(value) = stored.get("APP_ENCRYPTION_KEY") {
            resolved.insert("APP_ENCRYPTION_KEY".to_string(), value.clone());
        }
    }

    Ok(resolved)
}

/// Before every engine start (PLAN-B6 r3.3): the file's data and, on unix, its directory entry are on disk, whether
/// this launch wrote the file or found it. Nothing is written: bytes and mtime stay as they are.
pub fn make_durable(secrets_path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    let file = File::open(secrets_path);
    // FlushFileBuffers needs write access; no truncation, nothing written.
    #[cfg(not(unix))]
    let file = OpenOptions::new().write(true).open(secrets_path);
    file.and_then(|f| f.sync_all())
        .map_err(|error| format!("Failed to sync {:?}: {error}", secrets_path))?;
    sync_dir(secrets_path.parent().unwrap_or(Path::new(".")))
}

/// Test hooks between the steps of writing the file; nothing outside the unit tests.
pub(crate) mod hooks {
    use std::path::Path;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Stage {
        /// The temporary file exists, nothing written yet.
        Created,
        /// The file is linked in place, its directory not yet synced.
        Linked,
        /// A directory is about to be synced.
        SyncDir,
    }

    #[cfg(test)]
    type Hook = Box<dyn Fn(Stage, &Path) -> std::io::Result<()>>;

    #[cfg(test)]
    thread_local! {
        static HOOK: std::cell::RefCell<Option<Hook>> = const { std::cell::RefCell::new(None) };
        static SYNCED: std::cell::RefCell<Vec<std::path::PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
    }

    #[cfg(test)]
    pub fn set(hook: impl Fn(Stage, &Path) -> std::io::Result<()> + 'static) {
        HOOK.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
    }

    #[cfg(test)]
    pub fn clear() {
        HOOK.with(|h| *h.borrow_mut() = None);
        SYNCED.with(|s| s.borrow_mut().clear());
    }

    /// The directories synced on this thread since `clear`.
    #[cfg(test)]
    pub fn synced_dirs() -> Vec<std::path::PathBuf> {
        SYNCED.with(|s| s.borrow().clone())
    }

    #[cfg(test)]
    pub fn call(stage: Stage, path: &Path) -> std::io::Result<()> {
        HOOK.with(|h| h.borrow().as_ref().map_or(Ok(()), |hook| hook(stage, path)))
    }

    #[cfg(not(test))]
    pub fn call(_: Stage, _: &Path) -> std::io::Result<()> {
        Ok(())
    }

    #[cfg(test)]
    pub fn synced(dir: &Path) {
        SYNCED.with(|s| s.borrow_mut().push(dir.to_path_buf()));
    }

    #[cfg(not(test))]
    pub fn synced(_: &Path) {}
}
