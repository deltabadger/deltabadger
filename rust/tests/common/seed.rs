//! Rows shaped exactly as Rails writes them, for engine tests. Decimal values are text (SQLite's NUMERIC
//! affinity stores them as REAL, as it does for Rails), times use Rails' quoted_date format.
use deltabadger::crypto::{Cipher, EncryptionKeys};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

pub fn cipher() -> Cipher { Cipher::new(&EncryptionKeys::resolve(&|_| None, "engine-test-secret").unwrap()) }

/// What a seed created. `exchange_id`/`quote` are the seeded venue and quote asset (Kraken + EUR, or Alpaca + USD).
pub struct Seeded { pub user_id: i64, pub exchange_id: i64, pub btc: i64, pub quote: i64, pub ticker_id: i64, pub api_key_id: i64 }

const T: &str = "2026-01-01 00:00:00";
/// When the seeded Alpaca tickers were last synced: after every test clock, so no test but staleness's own reads them as
/// stale (a negative age is fresh). Kraken has no staleness bound and keeps T.
pub const SYNCED: &str = "2099-01-01 00:00:00";

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
    Seeded { user_id, exchange_id: kraken_id, btc, quote: eur, ticker_id, api_key_id }
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
    /// settings.allocations over [(asset id, weight)] in this order (the JSON column keeps it, as Rails' does).
    pub fn weights(self, weights: &[(i64, f64)]) -> Self {
        let m: serde_json::Map<String, Value> = weights.iter().map(|(id, w)| (id.to_string(), json!(w))).collect();
        self.with("allocations", Value::Object(m))
    }
}

pub fn insert_bot(c: &Connection, s: &Seeded, b: &BotSpec) -> i64 {
    let mut settings = b.settings.clone();
    settings["quote_asset_id"] = json!(s.quote);
    if settings.get("allocations").is_none() { settings["allocations"] = json!({ s.btc.to_string(): 1.0 }); }
    c.execute(
        "INSERT INTO bots (type, status, exchange_id, user_id, settings, transient_data, started_at, settings_changed_at, created_at, updated_at) \
         VALUES ('Bots::DcaMultiAsset', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        params![b.status, s.exchange_id, s.user_id, settings.to_string(), b.transient.to_string(), b.started_at, b.settings_changed_at, T]).unwrap();
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
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8, ?9, ?10, ?11, 'BTC', (SELECT symbol FROM assets WHERE id = ?13), ?12, ?13, 'week', 60, 'REGULAR', '[]', ?14, ?14)",
        params![bot_id, s.exchange_id, t.external_id, t.status, t.external_status, t.order_type, t.amount, t.quote_amount, t.price,
                t.amount_exec, t.quote_amount_exec, s.btc, s.quote, t.created_at]).unwrap();
    c.last_insert_rowid()
}

/// An Alpaca paper install: BTC/USD as data-api's listing sync imports it (base 9 / quote 2 / price 2 decimals,
/// minimum quote 1 USD), and a trading key whose passphrase is its mode, as the API-key form writes it.
pub fn seed_alpaca(c: &Connection, cipher: &Cipher) -> Seeded {
    c.execute("INSERT INTO users (email, encrypted_password, name, admin, created_at, updated_at) VALUES ('o@example.com', 'x', 'Owner', 1, ?1, ?1)", [T]).unwrap();
    let user_id = c.last_insert_rowid();
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Alpaca', 'Alpaca', '0.15', '0.25', ?1, ?1)", [T]).unwrap();
    let exchange_id = c.last_insert_rowid();
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('bitcoin', 'BTC', 'Bitcoin', 'Cryptocurrency', ?1, ?1)", [T]).unwrap();
    let btc = c.last_insert_rowid();
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('usd', 'USD', 'US Dollar', 'Currency', ?1, ?1)", [T]).unwrap();
    let quote = c.last_insert_rowid();
    c.execute(
        "INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
         minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) \
         VALUES (?1, 'BTC/USD', 'BTC', 'USD', ?2, ?3, 9, 2, 2, '0.000027', '1', 1, 1, ?4, ?5)",
        params![exchange_id, btc, quote, T, SYNCED]).unwrap();
    let ticker_id = c.last_insert_rowid();
    // The crypto catalog sync's stamp (staleness::ALPACA_CRYPTO_TICKERS): its ExchangeAsset rows and its last-good count.
    for asset in [btc, quote] {
        c.execute("INSERT INTO exchange_assets (exchange_id, asset_id, available, created_at, updated_at) VALUES (?1, ?2, 1, ?3, ?4)",
                  params![exchange_id, asset, T, SYNCED]).unwrap();
    }
    c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES ('alpaca_crypto_listings_last_good_count', '1', ?1, ?2)",
              params![T, SYNCED]).unwrap();
    c.execute(
        "INSERT INTO api_keys (user_id, exchange_id, key, secret, passphrase, status, key_type, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 1, 0, ?6, ?6)",
        params![user_id, exchange_id, cipher.encrypt("PKTEST"), cipher.encrypt("paper-secret"), cipher.encrypt("paper"), T]).unwrap();
    let api_key_id = c.last_insert_rowid();
    Seeded { user_id, exchange_id, btc, quote, ticker_id, api_key_id }
}

/// Another Alpaca crypto member beside seed_alpaca's BTC/USD: the asset (category Cryptocurrency) and SYMBOL/USD with this
/// precision ({"base_decimals", "quote_decimals", "price_decimals"} as numbers, {"minimum_base_size", "minimum_quote_size"}
/// as strings), synced at SYNCED. Returns (asset id, ticker id).
pub fn add_alpaca_crypto(c: &Connection, s: &Seeded, symbol: &str, pair: &Value) -> (i64, i64) {
    c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES (?1, ?2, ?2, 'Cryptocurrency', ?3, ?3)",
              params![format!("seed-{}", symbol.to_lowercase()), symbol, T]).unwrap();
    let asset = c.last_insert_rowid();
    let n = |k: &str| pair[k].as_i64().unwrap_or_else(|| panic!("{k} in {pair}"));
    let t = |k: &str| pair[k].as_str().unwrap_or_else(|| panic!("{k} in {pair}")).to_string();
    c.execute(
        "INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
         minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) \
         VALUES (?1, ?2, ?3, 'USD', ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, 1, ?11, ?12)",
        params![s.exchange_id, format!("{symbol}/USD"), symbol, asset, s.quote, n("base_decimals"), n("quote_decimals"), n("price_decimals"),
                t("minimum_base_size"), t("minimum_quote_size"), T, SYNCED]).unwrap();
    let ticker = c.last_insert_rowid();
    c.execute("INSERT INTO exchange_assets (exchange_id, asset_id, available, created_at, updated_at) VALUES (?1, ?2, 1, ?3, ?4)",
              params![s.exchange_id, asset, T, SYNCED]).unwrap();
    (asset, ticker)
}

/// ETH/USD and SOL/USD beside BTC/USD, with its precision and 1 USD minimum: (eth asset, sol asset).
pub fn add_eth_sol(c: &Connection, s: &Seeded) -> (i64, i64) {
    let pair = json!({ "base_decimals": 9, "quote_decimals": 2, "price_decimals": 2, "minimum_base_size": "0.000027", "minimum_quote_size": "1" });
    (add_alpaca_crypto(c, s, "ETH", &pair).0, add_alpaca_crypto(c, s, "SOL", &pair).0)
}

/// One transactions row as a recorded case lists it (`status`, `external_status`, `external_id`, `order_type`, and the
/// decimals `price`, `amount`, `quote_amount`, `amount_exec`, `quote_amount_exec` as strings; absent keys are NULL): a REGULAR
/// buy of `asset`. Decimals are bound as the Float Rails binds (BigDecimal#to_f), never as text for SQLite to convert.
pub fn insert_row(c: &Connection, s: &Seeded, bot_id: i64, asset: i64, row: &Value) -> i64 {
    let text = |k: &str| row.get(k).and_then(Value::as_str).map(str::to_string);
    let real = |k: &str| row.get(k).and_then(Value::as_str).map(|v| v.parse::<f64>().unwrap_or_else(|_| panic!("{k} {v}")));
    let int = |k: &str| row.get(k).and_then(Value::as_i64);
    c.execute(
        "INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, amount, quote_amount, price, \
         amount_exec, quote_amount_exec, base, quote, base_asset_id, quote_asset_id, bot_interval, bot_quote_amount, transaction_type, \
         error_messages, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8, ?9, ?10, ?11, (SELECT symbol FROM assets WHERE id = ?12), 'USD', ?12, ?13, 'day', 60, 'REGULAR', '[]', ?14, ?14)",
        params![bot_id, s.exchange_id, text("external_id"), int("status").unwrap_or(0), int("external_status"), int("order_type").unwrap_or(0),
                real("amount"), real("quote_amount"), real("price"), real("amount_exec"), real("quote_amount_exec"), asset, s.quote, text("created_at")]).unwrap();
    c.last_insert_rowid()
}
