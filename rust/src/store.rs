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
// Every table and column the engine reads or writes. Extend whenever Rust touches more.
const PRIMARY: &[TableContract] = &[
    TableContract { name: "action_mcp_sessions", columns: &[
        ("id", "varchar", true),
        ("client_capabilities", "json", false),
        ("client_info", "json", false),
        ("consents", "json", true),
        ("created_at", "datetime(6)", true),
        ("ended_at", "datetime(6)", false),
        ("initialized", "boolean", true),
        ("messages_count", "integer", true),
        ("prompt_registry", "json", false),
        ("protocol_version", "varchar", false),
        ("resource_registry", "json", false),
        ("role", "varchar", true),
        ("server_capabilities", "json", false),
        ("server_info", "json", false),
        ("session_data", "json", true),
        ("status", "varchar", true),
        ("tool_registry", "json", false),
        ("updated_at", "datetime(6)", true),
    ], unique_indexes: &[] },
    TableContract { name: "action_mcp_session_messages", columns: &[
        ("id", "integer", true),
        ("created_at", "datetime(6)", true),
        ("direction", "varchar", true),
        ("is_ping", "boolean", true),
        ("jsonrpc_id", "varchar", false),
        ("message_json", "json", false),
        ("message_type", "varchar", true),
        ("request_acknowledged", "boolean", true),
        ("request_cancelled", "boolean", true),
        ("session_id", "varchar", true),
        ("updated_at", "datetime(6)", true),
    ], unique_indexes: &[] },
    TableContract { name: "action_mcp_session_subscriptions", columns: &[
        ("id", "integer", true),
        ("created_at", "datetime(6)", true),
        ("last_notification_at", "datetime(6)", false),
        ("session_id", "varchar", true),
        ("updated_at", "datetime(6)", true),
        ("uri", "varchar", true),
    ], unique_indexes: &[] },
    TableContract { name: "app_configs", columns: &[("key", "varchar", true), ("value", "text", false), ("created_at", "datetime(6)", true), ("updated_at", "datetime(6)", true)], unique_indexes: &[&["key"]] },
    TableContract { name: "bots", columns: &[
        ("id", "integer", true), ("type", "varchar", false), ("status", "integer", true), ("exchange_id", "bigint", false),
        ("user_id", "bigint", false), ("label", "varchar", false), ("settings", "json", true), ("transient_data", "json", true),
        ("started_at", "datetime", false), ("stopped_at", "datetime", false), ("stop_message_key", "varchar", false),
        ("settings_changed_at", "datetime", false), ("last_end_of_funds_notification", "datetime", false),
        ("label", "varchar", false), ("position", "integer", true),
        ("updated_at", "datetime", true), ("restatement_generation", "integer", true),
    ], unique_indexes: &[] },
    TableContract { name: "transactions", columns: &[
        ("id", "integer", true), ("bot_id", "bigint", false), ("exchange_id", "bigint", true), ("external_id", "varchar", false),
        ("status", "integer", false), ("external_status", "integer", false), ("side", "integer", false), ("order_type", "integer", false),
        ("price", "decimal", false), ("amount", "decimal", false), ("quote_amount", "decimal", false),
        ("amount_exec", "decimal", false), ("quote_amount_exec", "decimal", false), ("base", "varchar", false), ("quote", "varchar", false),
        ("base_asset_id", "integer", false), ("quote_asset_id", "integer", false), ("bot_interval", "varchar", true),
        ("bot_quote_amount", "decimal", true), ("transaction_type", "varchar", true), ("error_messages", "json", true),
        ("created_at", "datetime", true), ("updated_at", "datetime", true),
    ], unique_indexes: &[&["external_id"]] },
    TableContract { name: "tickers", columns: &[
        ("id", "integer", true), ("exchange_id", "bigint", true), ("ticker", "varchar", true), ("base", "varchar", true),
        ("quote", "varchar", true), ("base_asset_id", "bigint", true), ("quote_asset_id", "bigint", true),
        ("base_decimals", "integer", true), ("quote_decimals", "integer", true), ("price_decimals", "integer", true),
        ("minimum_base_size", "decimal", true), ("minimum_quote_size", "decimal", true),
        ("maximum_base_size", "decimal", false), ("maximum_quote_size", "decimal", false),
        ("trading_enabled", "boolean", true), ("available", "boolean", false),
        ("created_at", "datetime(6)", true), ("updated_at", "datetime(6)", true),
    ], unique_indexes: &[&["exchange_id", "base_asset_id", "quote_asset_id"], &["exchange_id", "ticker"], &["exchange_id", "base", "quote"]] },
    // The bot pages (src/web/bot) also read an asset's name, colour, market cap and external id, and an exchange's fee and availability.
    TableContract { name: "assets", columns: &[
        ("id", "integer", true), ("external_id", "varchar", true), ("symbol", "varchar", false), ("name", "varchar", false),
        ("category", "varchar", false), ("instrument_type", "varchar", false), ("image_url", "varchar", false), ("color", "varchar", false),
        ("market_cap_rank", "integer", false), ("market_cap", "bigint", false), ("circulating_supply", "decimal(30,8)", false),
        ("url", "varchar", false), ("created_at", "datetime(6)", true), ("updated_at", "datetime(6)", true),
    ], unique_indexes: &[&["external_id"]] },
    TableContract { name: "exchanges", columns: &[
        ("id", "integer", true), ("type", "varchar", false), ("name", "varchar", false), ("maker_fee", "varchar", false), ("available", "boolean", false),
    ], unique_indexes: &[] },
    // The reference-data jobs (rust/src/jobs/import.rs) write exchange_assets and indices. `exchange.assets` is also what the
    // venue lists and the order balance rows are written in; staleness::ALPACA_CRYPTO_TICKERS reads updated_at: the catalog
    // sync's own stamp (MarketData.import_tickers!' ExchangeAsset upsert). An index bot's page draws the members of the
    // index it follows (src/web/bot/settings.rs).
    TableContract { name: "exchange_assets", columns: &[
        ("id", "integer", true), ("asset_id", "bigint", true), ("exchange_id", "bigint", true), ("available", "boolean", false),
        ("created_at", "datetime(6)", true), ("updated_at", "datetime(6)", true),
    ], unique_indexes: &[&["asset_id", "exchange_id"]] },
    TableContract { name: "indices", columns: &[
        ("id", "integer", true), ("external_id", "varchar", false), ("source", "varchar", false), ("name", "varchar", false),
        ("description", "text", false), ("top_coins", "json", false), ("top_coins_by_exchange", "json", false), ("market_cap", "decimal", false),
        ("available_exchanges", "json", false), ("weights", "json", false), ("weight", "integer", true),
        ("created_at", "datetime(6)", true), ("updated_at", "datetime(6)", true),
    ], unique_indexes: &[&["external_id", "source"]] },
    TableContract { name: "api_keys", columns: &[
        ("id", "integer", true), ("user_id", "bigint", true), ("exchange_id", "bigint", true), ("key", "varchar", false),
        ("secret", "varchar", false), ("passphrase", "varchar", false), ("status", "integer", true), ("key_type", "integer", true),
        // The tracker's syncs (src/sync): the ledger's watermark and error, the balances' clock.
        ("last_synced_at", "datetime(6)", false), ("last_sync_error", "varchar", false), ("balances_synced_at", "datetime(6)", false),
        ("updated_at", "datetime", true),
    ], unique_indexes: &[] },
    // What the ledger sync writes (src/sync/ledger.rs). Its dedup also relies on the partial unique index on
    // (user_id, exchange_id, tx_id), which this check cannot name (it lists unconditional indexes only).
    TableContract { name: "account_transactions", columns: &[
        ("id", "integer", true), ("user_id", "integer", true), ("api_key_id", "integer", false), ("exchange_id", "integer", true),
        ("entry_type", "integer", true), ("base_currency", "varchar", true), ("base_amount", "decimal", true),
        ("quote_currency", "varchar", false), ("quote_amount", "decimal", false), ("fee_currency", "varchar", false), ("fee_amount", "decimal", false),
        ("tx_id", "varchar", false), ("group_id", "varchar", false), ("description", "varchar", false), ("transacted_at", "datetime(6)", true),
        ("raw_data", "json", false), ("manual_values", "json", false), ("base_asset_id", "integer", false), ("transaction_id", "integer", false),
        ("linked_transaction_id", "integer", false), ("transfer_link_rejected", "boolean", true),
        ("created_at", "datetime(6)", true), ("updated_at", "datetime(6)", true),
    ], unique_indexes: &[&["linked_transaction_id"]] },
    // The web UI's sign-in (src/web/auth.rs) reads and writes the Devise columns; its layouts read the preferences.
    TableContract { name: "users", columns: &[
        ("id", "integer", true), ("wash_sale_enabled", "boolean", false), ("admin", "boolean", true), ("email", "varchar", true), ("name", "varchar", false),
        ("encrypted_password", "varchar", true), ("locale", "varchar", false), ("time_zone", "varchar", true),
        ("display_currency", "varchar", true), ("hide_balances", "boolean", true), ("confirmed_at", "datetime", false),
        ("failed_attempts", "integer", true), ("locked_at", "datetime(6)", false), ("otp_module", "integer", false),
        ("otp_secret_key", "varchar", false), ("last_otp_at", "datetime", false), ("remember_created_at", "datetime", false),
        ("updated_at", "datetime", true), ("tracker_settings", "json", false), ("mcp_settings", "json", false), ("rest_settings", "json", false),
    ], unique_indexes: &[&["email"]] },
    // Doorkeeper's tables and the per-client grant (src/web/oauth.rs, consent.rs, bearer.rs): written as Doorkeeper writes them.
    TableContract { name: "oauth_applications", columns: &[
        ("id", "integer", true), ("uid", "varchar", true), ("name", "varchar", true), ("secret", "varchar", false), ("redirect_uri", "text", false),
        ("scopes", "varchar", true), ("confidential", "boolean", true), ("personal_access_token", "boolean", true),
        ("personal_owner_id", "integer", false), ("registration_access_token", "varchar", false), ("token_endpoint_auth_method", "varchar", false), ("grant_types", "varchar", false),
        ("response_types", "varchar", false), ("created_at", "datetime(6)", true), ("updated_at", "datetime(6)", true),
    ], unique_indexes: &[&["uid"]] },
    TableContract { name: "oauth_access_grants", columns: &[
        ("id", "integer", true), ("application_id", "integer", true), ("resource_owner_id", "integer", true), ("token", "varchar", true),
        ("expires_in", "integer", true), ("redirect_uri", "text", true), ("scopes", "varchar", true), ("code_challenge", "varchar", false),
        ("code_challenge_method", "varchar", false), ("created_at", "datetime(6)", true), ("revoked_at", "datetime(6)", false),
    ], unique_indexes: &[&["token"]] },
    TableContract { name: "oauth_access_tokens", columns: &[
        ("id", "integer", true), ("application_id", "integer", true), ("resource_owner_id", "integer", false), ("token", "varchar", true),
        ("refresh_token", "varchar", false), ("previous_refresh_token", "varchar", true), ("scopes", "varchar", true), ("expires_in", "integer", false),
        ("created_at", "datetime(6)", true), ("revoked_at", "datetime(6)", false),
    ], unique_indexes: &[&["token"], &["refresh_token"]] },
    TableContract { name: "connected_clients", columns: &[
        ("id", "integer", true), ("user_id", "integer", true), ("oauth_application_id", "integer", true), ("mcp_tools", "json", true),
        ("rest_tools", "json", true), ("created_at", "datetime(6)", true), ("updated_at", "datetime(6)", true),
    ], unique_indexes: &[&["user_id", "oauth_application_id"]] },
    // The tick reads and writes a basket's members exactly as Bot::Composition::Allocatable does.
    TableContract { name: "bot_index_assets", columns: &[
        ("id", "integer", true), ("bot_id", "integer", true), ("asset_id", "integer", true), ("ticker_id", "integer", true),
        ("target_allocation", "decimal(10,6)", false), ("current_allocation", "decimal(10,6)", false), ("in_index", "boolean", false), ("entered_at", "datetime(6)", false),
        ("exited_at", "datetime(6)", false), ("created_at", "datetime(6)", true), ("updated_at", "datetime(6)", true),
    ], unique_indexes: &[&["bot_id", "asset_id"]] },
    // The balance sync writes every column and upserts on the unique index (src/sync/balances.rs); the bots page
    // refuses an account whose navbar would need the tracker ring (src/web/bots.rs).
    TableContract { name: "account_balances", columns: &[
        ("id", "integer", true), ("user_id", "integer", true), ("exchange_id", "integer", true), ("asset_id", "integer", true),
        ("free", "decimal(32,16)", true), ("locked", "decimal(32,16)", true), ("usd_price", "decimal(20,8)", false), ("usd_value", "decimal(20,8)", false),
        ("priced_at", "datetime(6)", false), ("synced_at", "datetime(6)", true), ("created_at", "datetime(6)", true), ("updated_at", "datetime(6)", true),
    ], unique_indexes: &[&["user_id", "exchange_id", "asset_id"]] },
    TableContract { name: "rules", columns: &[("id", "integer", true), ("status", "integer", true)], unique_indexes: &[] },
    TableContract { name: "bot_activity_logs", columns: &[
        ("id", "integer", true), ("bot_id", "integer", true), ("event", "varchar", true), ("level", "integer", true),
        ("details", "json", true), ("message", "varchar", false), ("created_at", "datetime(6)", true),
    ], unique_indexes: &[] },
];
const QUEUE: &[TableContract] = &[
    TableContract { name: "solid_queue_processes", columns: &[("last_heartbeat_at", "datetime(6)", true)], unique_indexes: &[] },
    TableContract { name: "solid_queue_jobs", columns: &[("id", "integer", true), ("class_name", "varchar", true), ("arguments", "text", false), ("finished_at", "datetime(6)", false)], unique_indexes: &[] },
];

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
