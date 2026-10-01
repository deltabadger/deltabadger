//! Rows shaped exactly as Rails writes them, for engine tests. Decimal values are text (SQLite's NUMERIC
//! affinity stores them as REAL, as it does for Rails), times use Rails' quoted_date format.
use deltabadger::crypto::{Cipher, EncryptionKeys};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

pub fn cipher() -> Cipher { Cipher::new(&EncryptionKeys::resolve(&|_| None, "engine-test-secret").unwrap()) }

pub struct Seeded { pub user_id: i64, pub kraken_id: i64, pub btc: i64, pub eur: i64, pub ticker_id: i64, pub api_key_id: i64 }

const T: &str = "2026-01-01 00:00:00";

pub fn seed_kraken(c: &Connection, cipher: &Cipher) -> Seeded {
    c.execute("INSERT INTO users (email, encrypted_password, name, admin, created_at, updated_at) VALUES ('o@example.com', 'x', 'Owner', 1, ?1, ?1)", [T]).unwrap();
    let user_id = c.last_insert_rowid();
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Kraken', 'Kraken', '0.25', '0.4', ?1, ?1)", [T]).unwrap();
    let kraken_id = c.last_insert_rowid();
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('bitcoin', 'BTC', 'Bitcoin', 'Cryptocurrency', ?1, ?1)", [T]).unwrap();
    let btc = c.last_insert_rowid();
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('EUR.FOREX', 'EUR', 'Euro', 'Currency', ?1, ?1)", [T]).unwrap();
    let eur = c.last_insert_rowid();
    c.execute(
        "INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
         minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) \
         VALUES (?1, 'XBTEUR', 'XBT', 'EUR', ?2, ?3, 8, 5, 1, '0.00005', '0.5', 1, 1, ?4, ?4)",
        params![kraken_id, btc, eur, T]).unwrap();
    let ticker_id = c.last_insert_rowid();
    c.execute(
        "INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 1, 0, ?5, ?5)",
        params![user_id, kraken_id, cipher.encrypt("test-key"), cipher.encrypt("dGVzdC1zZWNyZXQ="), T]).unwrap();
    let api_key_id = c.last_insert_rowid();
    Seeded { user_id, kraken_id, btc, eur, ticker_id, api_key_id }
}

pub struct BotSpec { pub status: i64, pub started_at: Option<String>, pub settings_changed_at: Option<String>, pub settings: Value, pub transient: Value }

impl BotSpec {
    /// A weekly one-asset basket, scheduled, started at `started_at`, market orders.
    pub fn weekly(quote_amount: f64, started_at: &str) -> Self {
        Self { status: 1, started_at: Some(started_at.into()), settings_changed_at: None,
               settings: json!({ "interval": "week", "quote_amount": quote_amount }), transient: json!({}) }
    }
    pub fn with(mut self, key: &str, value: Value) -> Self { self.settings[key] = value; self }
    pub fn transient(mut self, key: &str, value: Value) -> Self { self.transient[key] = value; self }
}

pub fn insert_bot(c: &Connection, s: &Seeded, b: &BotSpec) -> i64 {
    let mut settings = b.settings.clone();
    settings["quote_asset_id"] = json!(s.eur);
    if settings.get("allocations").is_none() { settings["allocations"] = json!({ s.btc.to_string(): 1.0 }); }
    c.execute(
        "INSERT INTO bots (type, status, exchange_id, user_id, settings, transient_data, started_at, settings_changed_at, created_at, updated_at) \
         VALUES ('Bots::DcaMultiAsset', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        params![b.status, s.kraken_id, s.user_id, settings.to_string(), b.transient.to_string(), b.started_at, b.settings_changed_at, T]).unwrap();
    c.last_insert_rowid()
}

pub struct TxSpec { pub status: i64, pub external_status: Option<i64>, pub external_id: Option<String>, pub order_type: i64,
                    pub amount: Option<&'static str>, pub quote_amount: Option<&'static str>, pub price: Option<&'static str>,
                    pub quote_amount_exec: Option<&'static str>, pub amount_exec: Option<&'static str>, pub created_at: String }

pub fn insert_tx(c: &Connection, s: &Seeded, bot_id: i64, t: &TxSpec) -> i64 {
    c.execute(
        "INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, amount, quote_amount, price, \
         amount_exec, quote_amount_exec, base, quote, base_asset_id, quote_asset_id, bot_interval, bot_quote_amount, transaction_type, \
         error_messages, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8, ?9, ?10, ?11, 'BTC', 'EUR', ?12, ?13, 'week', 60, 'REGULAR', '[]', ?14, ?14)",
        params![bot_id, s.kraken_id, t.external_id, t.status, t.external_status, t.order_type, t.amount, t.quote_amount, t.price,
                t.amount_exec, t.quote_amount_exec, s.btc, s.eur, t.created_at]).unwrap();
    c.last_insert_rowid()
}
