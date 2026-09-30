//! The four SQLite files the Rails app uses, created and upgraded only from Rails-generated SQL (db/sql).
//! Rust never invents schema. It checks first, read-only, and refuses a database it does not fully
//! understand before creating or changing any file. It builds a missing database in a temporary file
//! and renames it into place, so a crash never leaves a half-built install that looks complete.
use rusqlite::{Connection, OpenFlags};
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

include!(concat!(env!("OUT_DIR"), "/twins.rs"));

#[derive(Clone, Copy)]
pub struct Artifacts {
    pub primary_baseline: &'static str,
    pub twins: &'static [(&'static str, &'static str)],
    pub queue_baseline: &'static str,
    pub cache_baseline: &'static str,
    pub cable_baseline: &'static str,
    pub seed_gz: &'static [u8],
}

pub const EMBEDDED: Artifacts = Artifacts {
    primary_baseline: include_str!("../../db/sql/primary_baseline.sql"),
    twins: TWINS,
    queue_baseline: include_str!("../../db/sql/queue_baseline.sql"),
    cache_baseline: include_str!("../../db/sql/cache_baseline.sql"),
    cable_baseline: include_str!("../../db/sql/cable_baseline.sql"),
    seed_gz: include_bytes!("../../db/sql/seed.sql.gz"),
};

#[derive(Debug, Clone)]
pub struct Paths {
    pub primary: PathBuf,
    pub queue: PathBuf,
    pub cache: PathBuf,
    pub cable: PathBuf,
}

impl Paths {
    /// The variables config/database.yml reads, with Rails' production file names as defaults.
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>, storage_dir: &Path) -> Self {
        let pick = |var: &str, file: &str| env(var).filter(|v| !v.trim().is_empty()).map(PathBuf::from).unwrap_or_else(|| storage_dir.join(file));
        Self {
            primary: pick("DATABASE_PATH", "production.sqlite3"),
            queue: pick("QUEUE_DATABASE_PATH", "production_queue.sqlite3"),
            cache: pick("CACHE_DATABASE_PATH", "production_cache.sqlite3"),
            cable: pick("CABLE_DATABASE_PATH", "production_cable.sqlite3"),
        }
    }

    /// Next to the primary database, whatever the other paths are: the one file every engine agrees on.
    pub fn lock_file(&self) -> PathBuf {
        self.primary.parent().unwrap_or(Path::new(".")).join(".engine.lock")
    }
}

#[derive(Debug)]
pub enum StoreError {
    NewerSchema { unknown: Vec<String> },
    OlderSchema { missing: Vec<String> },
    /// A non-empty file with no schema_migrations: not ours to replace.
    Unrecognised { path: PathBuf },
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
}
impl From<rusqlite::Error> for StoreError { fn from(e: rusqlite::Error) -> Self { Self::Sqlite(e) } }
impl From<std::io::Error> for StoreError { fn from(e: std::io::Error) -> Self { Self::Io(e) } }

#[derive(Debug)]
pub enum Preflight {
    Create,
    Upgrade { pending: Vec<String> },
}

pub struct Opened {
    pub primary: Connection,
    pub queue: Connection,
    pub created: bool,
    pub applied_twins: Vec<String>,
}

/// What Rails sets on its SQLite connections (database.yml `timeout: 5000`, WAL, foreign keys).
pub fn configure(c: &Connection) -> rusqlite::Result<()> {
    c.busy_timeout(std::time::Duration::from_millis(5_000))?;
    c.pragma_update(None, "journal_mode", "WAL")?;
    c.pragma_update(None, "foreign_keys", "ON")
}

/// None for a missing or zero-byte file (both mean "create it"); otherwise a read-only connection.
/// A read-only connection can still create WAL sidecars; it never changes database contents.
fn read_only(path: &Path) -> Result<Option<Connection>, StoreError> {
    match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
        Ok(m) if m.len() == 0 => Ok(None),
        Ok(_) => Ok(Some(Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?)),
    }
}

/// true: an install database; false: missing (create it). A foreign non-empty file is an error.
fn recognised(path: &Path) -> Result<bool, StoreError> {
    match read_only(path)? {
        None => Ok(false),
        Some(c) if has_schema(&c)? => Ok(true),
        Some(_) => Err(StoreError::Unrecognised { path: path.to_path_buf() }),
    }
}

fn has_schema(c: &Connection) -> Result<bool, StoreError> {
    Ok(c.query_row("SELECT count(*) FROM sqlite_master WHERE name = 'schema_migrations'", [], |r| r.get::<_, i64>(0))? > 0)
}

fn baseline_versions(sql: &str) -> BTreeSet<String> {
    const MARK: &str = "INSERT INTO \"schema_migrations\" (\"version\") VALUES ('";
    sql.lines().filter_map(|l| l.strip_prefix(MARK)?.split('\'').next().map(str::to_string)).collect()
}

/// Read-only. Decides what `open` will do, and refuses a database this build cannot own.
pub fn preflight(paths: &Paths, a: &Artifacts) -> Result<Preflight, StoreError> {
    for aux in [&paths.queue, &paths.cache, &paths.cable] { recognised(aux)?; }
    if !recognised(&paths.primary)? { return Ok(Preflight::Create); }
    let c = read_only(&paths.primary)?.expect("recognised implies present");
    let have: BTreeSet<String> = {
        let mut s = c.prepare("SELECT version FROM schema_migrations")?;
        let v = s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        v
    };
    let baseline = baseline_versions(a.primary_baseline);
    let known: BTreeSet<String> = baseline.iter().cloned().chain(a.twins.iter().map(|(v, _)| v.to_string())).collect();
    let unknown: Vec<String> = have.difference(&known).cloned().collect();
    if !unknown.is_empty() { return Err(StoreError::NewerSchema { unknown }); }
    let missing: Vec<String> = baseline.difference(&have).cloned().collect();
    if !missing.is_empty() { return Err(StoreError::OlderSchema { missing }); }
    Ok(Preflight::Upgrade { pending: a.twins.iter().map(|(v, _)| v.to_string()).filter(|v| !have.contains(v)).collect() })
}

/// Builds `path` from SQL in a sibling temporary file, in one transaction, then renames it into place.
fn build(path: &Path, parts: &[&str], twins: &[(&str, &str)]) -> Result<(), StoreError> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.creating", path.file_name().unwrap().to_string_lossy()));
    let _ = std::fs::remove_file(&tmp);
    let result = (|| -> Result<(), StoreError> {
        let mut c = Connection::open(&tmp)?;
        c.pragma_update(None, "foreign_keys", "ON")?;
        let tx = c.transaction()?;
        tx.execute_batch(parts[0])?;
        for (version, sql) in twins {
            tx.execute_batch(sql)?;
            tx.execute("INSERT INTO schema_migrations (version) VALUES (?1)", [version])?;
        }
        for part in &parts[1..] { tx.execute_batch(part)?; }
        tx.commit()?;
        c.close().map_err(|(_, e)| e)?;
        Ok(())
    })();
    match result {
        Ok(()) => Ok(std::fs::rename(&tmp, path)?),
        Err(e) => { let _ = std::fs::remove_file(&tmp); Err(e) }
    }
}

pub fn open(paths: &Paths, a: &Artifacts) -> Result<Opened, StoreError> {
    let plan = preflight(paths, a)?; // every refusal happens here, before any file is created or changed

    for (path, baseline) in [(&paths.queue, a.queue_baseline), (&paths.cache, a.cache_baseline), (&paths.cable, a.cable_baseline)] {
        if !recognised(path)? { build(path, &[baseline], &[])?; }
    }

    let (created, applied_twins) = match plan {
        Preflight::Create => {
            let mut seed = String::new();
            flate2::read::GzDecoder::new(a.seed_gz).read_to_string(&mut seed)?;
            build(&paths.primary, &[a.primary_baseline, &seed], a.twins)?;
            (true, a.twins.iter().map(|(v, _)| v.to_string()).collect())
        }
        Preflight::Upgrade { pending } => {
            let mut c = Connection::open(&paths.primary)?;
            for (version, sql) in a.twins.iter().filter(|(v, _)| pending.iter().any(|p| p == v)) {
                let tx = c.transaction()?;
                tx.execute_batch(sql)?;
                tx.execute("INSERT INTO schema_migrations (version) VALUES (?1)", [version])?;
                tx.commit()?;
            }
            (false, pending)
        }
    };

    let primary = Connection::open(&paths.primary)?;
    configure(&primary)?;
    let queue = Connection::open(&paths.queue)?;
    configure(&queue)?;
    Ok(Opened { primary, queue, created, applied_twins })
}
