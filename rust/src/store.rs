//! Rails creates and migrates these databases until Rust owns the schema. Rust checks them
//! read-only and refuses anything that is not exactly what this build knows, before any write.
//! Rust never creates a database file.
use rusqlite::{Connection, OpenFlags};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

include!(concat!(env!("OUT_DIR"), "/migrations.rs"));

#[derive(Debug, Clone)]
pub struct Paths {
    pub primary: PathBuf,
    pub queue: PathBuf,
}

impl Paths {
    /// The variables config/database.yml reads, with Rails' production file names as defaults.
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>, storage_dir: &Path) -> Self {
        let pick = |var: &str, file: &str| {
            env(var)
                .filter(|v| !v.trim().is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| storage_dir.join(file))
        };
        Self {
            primary: pick("DATABASE_PATH", "production.sqlite3"),
            queue: pick("QUEUE_DATABASE_PATH", "production_queue.sqlite3"),
        }
    }

    /// Next to the primary database: the one lock file every engine agrees on.
    pub fn lock_file(&self) -> PathBuf {
        self.primary
            .parent()
            .unwrap_or(Path::new("."))
            .join(".engine.lock")
    }
}

#[derive(Debug)]
pub enum StoreError {
    /// A database file is missing or empty and must be prepared by Rails.
    Missing { path: PathBuf },
    /// A database has no schema_migrations table and is not recognised as an install.
    Unrecognised { path: PathBuf },
    /// The primary database lacks migrations required by this build.
    Behind { missing: Vec<String> },
    /// The primary database contains migrations unknown to this build.
    Unsupported { unknown: Vec<String> },
    /// The primary database has both missing and unknown migrations.
    Diverged {
        missing: Vec<String>,
        unknown: Vec<String>,
    },
    /// Tables used by Rust do not satisfy its structural contract.
    Incompatible { problems: Vec<String> },
    /// SQLite could not open or inspect a database or configure a connection.
    Sqlite(rusqlite::Error),
    /// A filesystem operation failed.
    Io(std::io::Error),
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}
impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

struct TableContract {
    name: &'static str,
    columns: &'static [(&'static str, &'static str, bool)],
    unique_indexes: &'static [&'static [&'static str]],
}

// Extend these contracts whenever Rust starts reading or writing another table or column.
// Checked before its rows are read as version strings.
const MIGRATIONS_TABLE: &[TableContract] = &[TableContract {
    name: "schema_migrations",
    columns: &[("version", "varchar", true)],
    unique_indexes: &[],
}];
const PRIMARY: &[TableContract] = &[
    TableContract {
        name: "app_configs",
        columns: &[
            ("key", "varchar", true),
            ("value", "text", false),
            ("created_at", "datetime(6)", true),
            ("updated_at", "datetime(6)", true),
        ],
        unique_indexes: &[&["key"]],
    },
];
const QUEUE: &[TableContract] = &[TableContract {
    name: "solid_queue_processes",
    columns: &[("last_heartbeat_at", "datetime(6)", true)],
    unique_indexes: &[],
}];

fn check_structure(
    c: &Connection,
    contract: &[TableContract],
    problems: &mut Vec<String>,
) -> rusqlite::Result<()> {
    for table in contract {
        let mut statement =
            c.prepare("SELECT name, type, \"notnull\" FROM pragma_table_xinfo(?1)")?;
        let columns = statement
            .query_map([table.name], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for &(name, declared_type, not_null) in table.columns {
            match columns.iter().find(|(column, _, _)| column == name) {
                None => problems.push(format!("{}.{} is missing", table.name, name)),
                Some((_, actual_type, actual_not_null)) => {
                    if !actual_type.eq_ignore_ascii_case(declared_type) {
                        problems.push(format!(
                            "{}.{} has type {actual_type}; expected {declared_type}",
                            table.name, name
                        ));
                    }
                    if *actual_not_null != not_null {
                        let expected = if not_null { "NOT NULL" } else { "nullable" };
                        problems.push(format!("{}.{} must be {expected}", table.name, name));
                    }
                }
            }
        }
        if table.unique_indexes.is_empty() {
            continue;
        }
        // A partial unique index cannot support an unconditional ON CONFLICT column target.
        let mut statement = c.prepare(
            "SELECT name FROM pragma_index_list(?1) WHERE \"unique\" = 1 AND partial = 0",
        )?;
        let indexes = statement
            .query_map([table.name], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut indexed_columns = Vec::new();
        for index in indexes {
            let mut statement =
                c.prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno")?;
            indexed_columns.push(
                statement
                    .query_map([index], |r| r.get::<_, Option<String>>(0))?
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
        for required in table.unique_indexes {
            if !indexed_columns.iter().any(|columns| {
                columns
                    .iter()
                    .map(|c| c.as_deref())
                    .eq(required.iter().copied().map(Some))
            }) {
                problems.push(format!(
                    "{} ({}) is missing a unique index",
                    table.name,
                    required.join(", ")
                ));
            }
        }
    }
    Ok(())
}

fn read_only(path: &Path) -> Result<Connection, StoreError> {
    match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(StoreError::Missing {
                path: path.to_path_buf(),
            })
        }
        Err(e) => return Err(e.into()),
        Ok(m) if m.len() == 0 => {
            return Err(StoreError::Missing {
                path: path.to_path_buf(),
            })
        }
        Ok(_) => {}
    }
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let recognised: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations')",
        [], |r| r.get(0),
    )?;
    if !recognised {
        return Err(StoreError::Unrecognised {
            path: path.to_path_buf(),
        });
    }
    Ok(c)
}

/// Checks both databases read-only, without creating files or applying migrations.
pub fn check(paths: &Paths) -> Result<(), StoreError> {
    let primary = read_only(&paths.primary)?;
    let queue = read_only(&paths.queue)?;
    let mut problems = Vec::new();
    check_structure(&primary, MIGRATIONS_TABLE, &mut problems)?;

    // Only read migration values if their column is compatible with the expected representation.
    if problems.is_empty() {
        let mut statement = primary.prepare("SELECT version FROM schema_migrations")?;
        let have = statement
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<BTreeSet<_>, _>>()?;
        let known: BTreeSet<String> = MIGRATIONS.iter().map(|v| v.to_string()).collect();
        let missing: Vec<String> = known.difference(&have).cloned().collect();
        let unknown: Vec<String> = have.difference(&known).cloned().collect();
        match (missing.is_empty(), unknown.is_empty()) {
            (false, true) => return Err(StoreError::Behind { missing }),
            (true, false) => return Err(StoreError::Unsupported { unknown }),
            (false, false) => return Err(StoreError::Diverged { missing, unknown }),
            (true, true) => {}
        }
    }
    check_structure(&primary, PRIMARY, &mut problems)?;
    check_structure(&queue, QUEUE, &mut problems)?;
    if !problems.is_empty() {
        return Err(StoreError::Incompatible { problems });
    }
    Ok(())
}

pub struct Opened {
    pub primary: Connection,
    pub queue: Connection,
}

/// Opens an install for writes only after both databases pass the read-only check.
pub fn open(paths: &Paths) -> Result<Opened, StoreError> {
    check(paths)?;
    let primary = Connection::open_with_flags(&paths.primary, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let queue = Connection::open_with_flags(&paths.queue, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    configure(&primary)?;
    configure(&queue)?;
    Ok(Opened { primary, queue })
}

/// Sets the connection timeout and foreign keys; Rails owns the journal mode.
pub fn configure(c: &Connection) -> rusqlite::Result<()> {
    c.busy_timeout(std::time::Duration::from_millis(5_000))?;
    c.pragma_update(None, "foreign_keys", "ON")
}
