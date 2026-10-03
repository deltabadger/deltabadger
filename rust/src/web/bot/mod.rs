//! A bot as the bot list and the bot page read it: the row, what Rails' model derives from it
//! (Bots::DcaMultiAsset, Bots::DcaIndex and their concerns), and what this build does not render.
//! Read-only: nothing here writes.
pub mod page;
pub mod settings;
pub mod start;
pub mod status;

use super::format::Num;
use super::WebError;
use crate::codec::parse_time;
use crate::engine::schedule::{self, Checkpoints, Effective, Interval, Unrounded};
use crate::engine::EngineError;
use crate::enums::{ApiKeyStatus, BotStatus, BOT_WORKING};
use crate::ruby::{scale, BigDec};
pub use super::format::{stored, unreadable, Stored, Unreadable, UNREADABLE};
use chrono::{DateTime, Utc};
use rusqlite::types::{FromSql, FromSqlResult, ValueRef};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Map, Value};

/// Bots::DcaMultiAsset::MAX_ASSETS.
pub const MAX_ASSETS: usize = 100;
/// Bots::DcaIndex::MIN_COINS and MAX_COINS.
pub const MIN_COINS: i64 = 2;
pub const MAX_COINS: i64 = 100;
/// The most decimals a ticker may state and still be rendered. The pages round to a ticker's
/// decimals, and the column has no bound of its own (db/schema.rb, app/models/ticker.rb). 14 is
/// where the bound is because it is as far as `format::float_round` is Ruby's: past 14 digits MRI
/// rounds a Float through a Rational (float.c, `rb_float_round`), which is not ported. It is inside
/// `ruby::MAX_SCALE`, the engine's bound on a rounding scale, and tighter only because the pages
/// round Floats, which the engine never does. Real tickers go past it: MEXC lists two with 15
/// (db/seed_data/tickers/mexc.json), on a venue these pages do not serve yet. On Alpaca, the one
/// venue served, stocks are written with 9 and 2 (app/models/exchanges/alpaca.rb) and crypto with
/// the decimals of the step the venue states (app/jobs/exchange/sync_alpaca_assets_job.rb,
/// app/models/market_data.rb), which nothing bounds: an Alpaca ticker past 14 is refused like any
/// other, until the Rational path is ported.
pub const MAX_DECIMALS: i64 = 14;
/// The longest a bot may wait between two orders and still be rendered: a thousand years of
/// 365.2425 days. Smart Intervals stretch the interval by `smart amount / amount`, and Rails bounds
/// that amount from below only, so a row can state any span. Rails prints it in words whatever it is
/// (Ruby's Time has no last year); the calendar here ends near the year 262,000, and a checkpoint
/// is held as microseconds in an i64, which end near the year 294,000. Nobody waits a thousand years
/// for an order.
pub const MAX_SPAN_SECONDS: f64 = 31_556_952_000.0;
/// The shortest span between two orders of a working bot that is still rendered: one second. Rails
/// starts no bot whose Smart Intervals leave less than five minutes, but checks that at the start
/// only, and a row can state any amount: zero and a negative one give a span of nothing or less,
/// and one small enough underflows to it. The checkpoints of such a span are not numbers (a
/// quotient by zero), so a working bot below this is refused before any is computed. A bot that is
/// not working has no checkpoint to compute, and Rails prints its own error under the field.
pub const MIN_SPAN_SECONDS: f64 = 1.0;

fn data(message: String) -> WebError {
    WebError::Engine(EngineError::Data(message))
}

/// `tickers.base_decimals` and `quote_decimals` as a rounding scale: the only way a precision out
/// of the database reaches a rounding here (`ruby::scale`).
struct Scale(u8);

impl FromSql for Scale {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        scale(value.as_i64()?).map(Scale).or_else(super::format::unread)
    }
}

/// ActiveModel::Type::Boolean#cast of a stored value: nil for nothing and for "", false for its
/// list of false spellings, true for anything else.
fn cast_boolean(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null | Value::Bool(false)) => false,
        Some(Value::String(text)) => !["", "0", "f", "F", "false", "FALSE", "off", "OFF"].contains(&text.as_str()),
        Some(Value::Number(number)) => number.as_i64() != Some(0),
        Some(_) => true,
    }
}

/// A text Ruby's `to_f` and `to_d` and this crate read as the same number: digits, with a sign
/// before them and a fraction after them or without. Rails coerces a setting it reads with either
/// from any text ("0.6abc" is 0.6, "abc" is 0); the port follows it where the two cannot differ
/// and refuses the rest.
fn plain_decimal(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, "0"));
    let all_digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    text.len() <= crate::ruby::MAX_INPUT_LEN && all_digits(whole) && all_digits(fraction)
}

/// `allocations[id].to_f`: a weight as a Float, from a number or from a text that is plainly one.
fn weight(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) if plain_decimal(text) => text.parse().ok(),
        _ => None,
    }
}

#[derive(Clone, Debug)]
pub struct Asset {
    pub id: i64,
    pub symbol: Option<String>,
    pub name: Option<String>,
    pub color: Option<String>,
    pub category: Option<String>,
    pub external_id: String,
    pub market_cap: Option<i64>,
}

impl Asset {
    const COLUMNS: &'static str = "id, symbol, name, color, category, external_id, market_cap";

    fn read(r: &rusqlite::Row<'_>) -> rusqlite::Result<Asset> {
        Ok(Asset { id: r.get(0)?, symbol: r.get(1)?, name: r.get(2)?, color: r.get(3)?, category: r.get(4)?, external_id: r.get(5)?, market_cap: r.get(6)? })
    }

    pub fn find(c: &Connection, id: i64) -> Result<Option<Asset>, WebError> {
        Ok(c.query_row(&format!("SELECT {} FROM assets WHERE id = ?1", Self::COLUMNS), [id], Self::read).optional()?)
    }

    pub fn find_by_external_id(c: &Connection, external_id: &str) -> Result<Option<Asset>, WebError> {
        Ok(c.query_row(&format!("SELECT {} FROM assets WHERE external_id = ?1 ORDER BY id LIMIT 1", Self::COLUMNS), [external_id], Self::read).optional()?)
    }

    pub fn symbol(&self) -> &str {
        self.symbol.as_deref().unwrap_or("")
    }
}

#[derive(Clone, Debug)]
pub struct Ticker {
    pub id: i64,
    pub exchange_id: i64,
    /// The venue's own spelling of the base.
    pub base: String,
    pub base_asset_id: i64,
    pub quote_asset_id: i64,
    pub base_decimals: u8,
    pub quote_decimals: u8,
    pub minimum_quote_size: BigDec,
    pub available: bool,
    pub trading_enabled: bool,
    pub base_symbol: Option<String>,
    pub quote_symbol: Option<String>,
}

impl Ticker {
    const SELECT: &'static str = "SELECT t.id, t.exchange_id, t.base, t.base_asset_id, t.quote_asset_id, t.base_decimals, t.quote_decimals, \
                                  t.minimum_quote_size, t.available, t.trading_enabled, b.symbol, q.symbol \
                                  FROM tickers t LEFT JOIN assets b ON b.id = t.base_asset_id LEFT JOIN assets q ON q.id = t.quote_asset_id";

    fn read(r: &rusqlite::Row<'_>) -> rusqlite::Result<Ticker> {
        Ok(Ticker {
            id: r.get(0)?, exchange_id: r.get(1)?, base: r.get(2)?, base_asset_id: r.get(3)?, quote_asset_id: r.get(4)?, base_decimals: r.get::<_, Scale>(5)?.0,
            quote_decimals: r.get::<_, Scale>(6)?.0, minimum_quote_size: r.get::<_, Stored>(7)?.0.unwrap_or_else(BigDec::zero),
            // Ticker#available? on a NULL column is false.
            available: r.get::<_, Option<bool>>(8)?.unwrap_or(false), trading_enabled: r.get::<_, Option<bool>>(9)?.unwrap_or(false),
            base_symbol: r.get(10)?, quote_symbol: r.get(11)?,
        })
    }

    fn all(c: &Connection, condition: &str, values: &[&dyn rusqlite::ToSql]) -> Result<Vec<Ticker>, WebError> {
        let mut statement = c.prepare(&format!("{} WHERE {condition} ORDER BY t.id", Self::SELECT))?;
        let rows = statement.query_map(values, Self::read)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn tradable(&self) -> bool {
        self.available && self.trading_enabled
    }
}

#[derive(Clone, Debug)]
pub struct Exchange {
    pub id: i64,
    pub name: String,
    /// `exchanges.type`: Exchanges::Alpaca.
    pub class: String,
    pub maker_fee: Option<String>,
}

impl Exchange {
    /// Exchange#name_id: `self.class.name.demodulize.underscore` ("alpaca", "binance_us").
    pub fn name_id(&self) -> String {
        let name = self.class.rsplit("::").next().unwrap_or(&self.class);
        let mut out = String::new();
        for (index, c) in name.chars().enumerate() {
            if c.is_ascii_uppercase() && index > 0 { out.push('_'); }
            out.push(c.to_ascii_lowercase());
        }
        out
    }

    /// Exchange::RETIRED_TYPES.
    pub fn retired(&self) -> bool {
        self.class == "Exchanges::Bitmart"
    }
}

/// A `bot_index_assets` row: a member of the composition, or one that left it.
#[derive(Clone, Debug)]
pub struct Membership {
    pub asset: Asset,
    pub ticker: Option<Ticker>,
    pub target_allocation: Option<BigDec>,
    pub in_index: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Bots::DcaMultiAsset: a basket of one or more assets at the user's weights.
    Basket,
    /// Bots::DcaIndex.
    Index,
}

/// The `indices` row an index bot follows (Bots::DcaIndex#current_index), as far as the page reads it.
#[derive(Clone, Debug, Default)]
pub struct IndexRow {
    pub source: Option<String>,
    pub top_coins: Vec<String>,
    pub weights: Map<String, Value>,
}

/// The last order of a bot (`bot.transactions.last`), for the status bar of a retrying bot.
#[derive(Clone, Debug)]
pub struct LastOrder {
    pub failed: bool,
    pub error_messages: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Bot {
    pub id: i64,
    pub kind: Kind,
    pub status: BotStatus,
    pub label: String,
    pub exchange: Exchange,
    pub settings: Map<String, Value>,
    pub transient: Map<String, Value>,
    pub started_at: Option<DateTime<Utc>>,
    pub stop_message_key: Option<String>,
    pub quote_asset: Option<Asset>,
    /// A basket's members in the order of its `allocations` (Bots::DcaMultiAsset::Allocatable#base_assets).
    pub base_assets: Vec<Asset>,
    /// Every `bot_index_assets` row, in id order.
    pub memberships: Vec<Membership>,
    /// `bot.tickers` (each type's set_tickers): what the bot can price and trade here.
    pub tickers: Vec<Ticker>,
    /// The status of the user's trading key on this exchange; `None` without one (Rails then builds a pending key).
    pub api_key: Option<i64>,
    pub index: Option<IndexRow>,
    pub last_order: Option<LastOrder>,
    /// `bot.transactions.any?`, `.waiting.exists?`, `.regular.waiting.exists?`, `.submitted.exists?`.
    pub has_orders: bool,
    pub has_waiting_orders: bool,
    pub has_regular_waiting_orders: bool,
    pub has_submitted_orders: bool,
}

/// What a setting the pages read may be stored as. Rails validates each on every save, so a row a
/// form wrote always fits; a row something else wrote may not, and Rails then prints what its own
/// reader makes of it. This build reads the stored shape and nothing else: an absent or null
/// setting takes Rails' default, and any other shape is refused, never read as a default or as zero.
enum Shape {
    /// A JSON number.
    Number,
    /// A JSON number from 0 to 1.
    Share,
    /// `true` or `false`: Rails validates every switch as exactly one of the two.
    Flag,
    /// `missed_quote_amount`, which Rails reads with `to_d` and validates as not negative: a number, or a decimal text within `ruby::BigDec`'s bounds.
    Carry,
    /// A JSON number without a fraction.
    Integer,
    /// A JSON string.
    Text,
    /// A JSON string that is one of these.
    OneOf(&'static [&'static str]),
    /// A ticker's id: an integer, or the digits of one (Rails looks it up either way).
    Id,
    /// A time as Rails writes one into a JSON column (`Time#as_json`), or the empty text Rails reads as none.
    Time,
    /// `rebalance_threshold`, which Rails reads with `presence&.to_d` and validates as above 0 and at most 1: such a number, a text that is plainly one, or nothing (false, a blank text).
    Threshold,
}

const TIMINGS: &[&str] = &["while", "after"];
const SIDES: &[&str] = &["above", "below"];
const BUY_ACTIONS: &[&str] = &["pause", "start_selling"];
const TIMEFRAMES: &[&str] = &["one_hour", "four_hours", "one_day", "three_days", "one_week", "one_month"];

/// Every setting the two pages read, or Rails validates when it is asked whether the bot may start,
/// beyond the few `refusal` checks by hand; for both bot types (a type that has no such setting has
/// no such key). Compared with every `validates` of the two classes and their concerns that runs in
/// the default or the `:start` context, and with every attribute the templates read. The two switches
/// Rails reads by a boolean cast and does not validate (`start_time_enabled`, `hold_all`) are read
/// the same way here, whatever is stored.
const SETTINGS: &[(&str, Shape)] = &[
    ("smart_intervaled", Shape::Flag), ("limit_ordered", Shape::Flag), ("quote_amount_limited", Shape::Flag), ("price_limited", Shape::Flag),
    ("price_drop_limited", Shape::Flag), ("moving_average_limited", Shape::Flag), ("indicator_limited", Shape::Flag),
    ("weighting", Shape::OneOf(&["manual", "market_cap"])), ("direction", Shape::OneOf(&["buying", "selling"])),
    ("indicator_limit_in_indicator", Shape::OneOf(&["rsi"])),
    ("start_at", Shape::Text), ("start_time_of_day", Shape::Text), ("rebalance_threshold", Shape::Threshold),
    ("quote_amount_limit", Shape::Number),
    ("price_limit", Shape::Number), ("price_limit_range_lower_bound", Shape::Number), ("price_limit_range_upper_bound", Shape::Number),
    ("price_limit_timing_condition", Shape::OneOf(TIMINGS)), ("price_limit_value_condition", Shape::OneOf(&["above", "below", "between"])),
    ("price_limit_action", Shape::OneOf(BUY_ACTIONS)), ("price_limit_in_ticker_id", Shape::Id),
    ("price_drop_limit", Shape::Number), ("price_drop_limit_time_window_condition", Shape::OneOf(&["ath", "twenty_four_hours"])),
    ("price_drop_limit_action", Shape::OneOf(BUY_ACTIONS)), ("price_drop_limit_in_ticker_id", Shape::Id),
    ("moving_average_limit_timing_condition", Shape::OneOf(TIMINGS)), ("moving_average_limit_value_condition", Shape::OneOf(SIDES)),
    ("moving_average_limit_in_ma_type", Shape::OneOf(&["sma", "ema"])), ("moving_average_limit_in_timeframe", Shape::OneOf(TIMEFRAMES)),
    ("moving_average_limit_in_period", Shape::Integer), ("moving_average_limit_action", Shape::OneOf(BUY_ACTIONS)), ("moving_average_limit_in_ticker_id", Shape::Id),
    ("indicator_limit", Shape::Number), ("indicator_limit_timing_condition", Shape::OneOf(TIMINGS)), ("indicator_limit_value_condition", Shape::OneOf(SIDES)),
    ("indicator_limit_in_timeframe", Shape::OneOf(TIMEFRAMES)), ("indicator_limit_action", Shape::OneOf(BUY_ACTIONS)), ("indicator_limit_in_ticker_id", Shape::Id),
    ("index_type", Shape::OneOf(&["top", "category"])), ("index_category_id", Shape::Text), ("index_name", Shape::Text), ("num_coins", Shape::Integer),
    ("allocation_flattening", Shape::Share),
];
/// And of `transient_data`: the two times the pages read, and the carry Rails validates.
const TRANSIENT: &[(&str, Shape)] = &[("last_action_job_at", Shape::Time), ("quote_amount_limit_enabled_at", Shape::Time), ("missed_quote_amount", Shape::Carry)];

/// The number a text that is plainly a decimal states, as a Float.
fn plain(text: &str) -> Option<f64> {
    if plain_decimal(text) { text.parse().ok() } else { None }
}

/// What Rails validates only while a rule is switched on (`if: :price_limited?` and its like): a
/// row may hold anything there while the rule is off, and Rails renders it; switched on, a value
/// outside the range is an error under its field, which this build does not print, so it refuses.
fn ranges_hold(settings: &Value) -> bool {
    let on = |key: &str| settings.get(key) == Some(&Value::Bool(true));
    let number = |key: &str| settings.get(key).and_then(Value::as_f64);
    let not_negative = |key: &&str| number(key).is_none_or(|n| n >= 0.0);
    (!on("price_limited") || ["price_limit", "price_limit_range_lower_bound", "price_limit_range_upper_bound"].iter().all(not_negative))
        && (!on("price_drop_limited") || number("price_drop_limit").is_none_or(|share| (0.0..=1.0).contains(&share)))
        && (!on("moving_average_limited") || settings.get("moving_average_limit_in_period").and_then(Value::as_i64).is_none_or(|period| period > 0))
}

fn fits(value: &Value, shape: &Shape) -> bool {
    match shape {
        Shape::Number => value.is_number(),
        Shape::Share => value.as_f64().is_some_and(|share| (0.0..=1.0).contains(&share)),
        Shape::Flag => value.is_boolean(),
        // Rails writes the carry as a BigDecimal, which a JSON column holds as its text ("9999999999999999999999999999900.0"
        // after a budget of 1e31): any text `ruby::BigDec` reads within its bounds, not negative.
        Shape::Carry => match value {
            Value::Number(number) => number.as_f64().is_some_and(|carry| carry >= 0.0),
            Value::String(text) => text.is_empty() || BigDec::parse(text).is_ok_and(|carry| carry >= BigDec::zero()),
            _ => false,
        },
        Shape::Integer => value.as_i64().is_some(),
        Shape::Text => value.is_string(),
        Shape::OneOf(words) => value.as_str().is_some_and(|text| words.contains(&text)),
        Shape::Id => value.as_i64().is_some() || value.as_str().is_some_and(|text| !text.is_empty() && text.len() <= 18 && text.bytes().all(|b| b.is_ascii_digit())),
        Shape::Time => value.as_str().is_some_and(|text| text.is_empty() || DateTime::parse_from_rfc3339(text).is_ok()),
        Shape::Threshold => match value {
            Value::Bool(false) => true,
            Value::Number(number) => number.as_f64().is_some_and(|share| share > 0.0 && share <= 1.0),
            Value::String(text) => text.trim().is_empty() || plain(text).is_some_and(|share| share > 0.0 && share <= 1.0),
            _ => false,
        },
    }
}

fn shaped(values: &Value, shapes: &[(&str, Shape)]) -> bool {
    shapes.iter().all(|(key, shape)| values.get(key).is_none_or(|value| value.is_null() || fits(value, shape)))
}

/// A basket's `allocations` as the pages can read them: asset ids as Rails writes them (`"12"`,
/// never `"012"` or `"12abc"`, which `to_i` would still read) and weights that are numbers or
/// plainly numbers. `base_asset_ids`, which a bot the wizard has not finished carries instead, are integers.
fn members_shaped(settings: &Value) -> bool {
    let id = |key: &String| key.parse::<i64>().is_ok_and(|id| id > 0 && id.to_string() == *key);
    match settings.get("allocations") {
        Some(Value::Object(weights)) if !weights.is_empty() => weights.iter().all(|(key, value)| id(key) && weight(value).is_some()),
        Some(Value::Object(_)) => settings.get("base_asset_ids").is_none_or(|ids| ids.is_null() || ids.as_array().is_some_and(|ids| ids.iter().all(|id| id.as_i64().is_some()))),
        _ => false,
    }
}

/// Which request asks: the feed answers many times for one page, and so is not made to walk a
/// bot's whole history each time.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum For { Page, Feed }

/// Why a bot's pages are not served by this build. Rails serves each of these; porting what
/// a line names removes it.
pub fn refusal(c: &Connection, bot_id: i64, wash_sale_enabled: Option<bool>, provider_is_deltabadger: bool, asked: For) -> Result<Option<&'static str>, WebError> {
    let row = c.query_row("SELECT b.type, e.type, b.label, b.settings, b.transient_data FROM bots b LEFT JOIN exchanges e ON e.id = b.exchange_id WHERE b.id = ?1",
                          [bot_id], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?)))?;
    let (class, exchange, label, settings, transient) = row;
    let settings: Value = serde_json::from_str(&settings).unwrap_or(Value::Null);
    let transient: Value = serde_json::from_str(&transient).unwrap_or(Value::Null);
    let truthy = |value: Option<&Value>| value.is_some_and(|v| !matches!(v, Value::Null | Value::Bool(false)) && v.as_str() != Some("") && v.as_str() != Some("0") && v.as_str() != Some("false"));
    let class = class.unwrap_or_default();
    if class != "Bots::DcaMultiAsset" && class != "Bots::DcaIndex" { return Ok(Some("a bot of a type this build does not render")); }
    if exchange.as_deref() != Some("Exchanges::Alpaca") { return Ok(Some("a bot on an exchange other than Alpaca")); }
    // Automation::Labelable#ensure_label_exists writes a label when a page finds none.
    if label.is_none_or(|label| label.trim().is_empty()) { return Ok(Some("a bot without a label, which Rails writes on load")); }
    // What the wizard always stores and no concern has a default for. Every other setting a row may
    // lack is given the value Rails gives it on load (`Bot::fill_defaults`).
    let needed: &[&str] = if class == "Bots::DcaIndex" { &["quote_asset_id", "interval", "index_type"] } else { &["quote_asset_id", "interval", "allocations"] };
    if needed.iter().any(|key| settings.get(key).is_none_or(Value::is_null)) { return Ok(Some("settings the wizard always stores are missing")); }
    // Shapes Rails' readers would coerce or trip over, each in its own way: read as stored, or not at all.
    let members = class == "Bots::DcaIndex" || members_shaped(&settings);
    if settings.get("quote_asset_id").and_then(Value::as_i64).is_none() || !members || !shaped(&settings, SETTINGS) || !shaped(&transient, TRANSIENT) || !ranges_hold(&settings) {
        return Ok(Some("a setting stored in a shape this build does not read"));
    }
    // Shapes no form saves. Rails would print each as an error under its field; nothing can have stored them but a migration.
    let positive = |key: &str| settings.get(key).and_then(Value::as_f64).is_some_and(|number| number > 0.0);
    // `||=` in Ruby: a setting that is absent, null or false takes its default.
    let defaulted = |key: &str| matches!(settings.get(key), None | Some(Value::Null | Value::Bool(false)));
    let mode_known = settings.get("start_time_mode").is_none_or(|mode| mode.is_null() || mode.as_str().is_some_and(|mode| mode.is_empty() || start::MODES.contains(&mode)));
    let smart_ok = defaulted("smart_interval_quote_amount") || settings.get("smart_interval_quote_amount").is_some_and(Value::is_number);
    let distance_ok = defaulted("limit_order_pcnt_distance") || settings.get("limit_order_pcnt_distance").and_then(Value::as_f64).is_some_and(|share| (0.0..=1.0).contains(&share));
    let interval_ok = matches!(settings.get("interval").and_then(Value::as_str), Some("hour" | "day" | "week" | "month"));
    if !positive("quote_amount") || !interval_ok || !mode_known || !smart_ok || !distance_ok { return Ok(Some("settings no form would have saved")); }
    if settings.get("direction").and_then(Value::as_str) == Some("selling") { return Ok(Some("a selling bot")); }
    if truthy(settings.get("rebalance_enabled")) { return Ok(Some("rebalancing")); }
    if settings.get("weighting").and_then(Value::as_str) == Some("market_cap") { return Ok(Some("market-cap weights")); }
    if ["rebalance_pending", "liquidation_pending", "liquidation_selling_since", "redeploy_pending"].iter().any(|key| transient.get(key).is_some_and(|v| !v.is_null())) {
        return Ok(Some("a rebalance, liquidation or redeploy in progress"));
    }
    if class == "Bots::DcaIndex" && !provider_is_deltabadger { return Ok(Some("an index bot whose market data comes from CoinGecko")); }
    // Every ticker the pages may round by: the venue's, and those the bot's memberships name.
    let unbounded: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM tickers t WHERE (t.exchange_id = (SELECT exchange_id FROM bots WHERE id = ?1) OR t.id IN (SELECT ticker_id FROM bot_index_assets WHERE bot_id = ?1)) \
         AND (t.base_decimals NOT BETWEEN 0 AND ?2 OR t.quote_decimals NOT BETWEEN 0 AND ?2))", (bot_id, MAX_DECIMALS), |r| r.get(0))?;
    if unbounded { return Ok(Some("a ticker with more decimals than this build rounds to")); }
    // The metrics walk, not ported yet, is what reads sells, swaps and corporate actions. The page asks once: an order of
    // another type through index_bot_type_created_at (bot_id, transaction_type, created_at), on either side of 'REGULAR',
    // and a sell by walking the bot's orders (no index tells one from a buy). The feed asks neither: Rails picks its ten
    // rows first, and `orders::feed` refuses the page whose rows hold such an order, and no other.
    let other_type = asked == For::Page && c.query_row("SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id = ?1 AND transaction_type < 'REGULAR') \
                                        OR EXISTS(SELECT 1 FROM transactions WHERE bot_id = ?1 AND transaction_type > 'REGULAR')", [bot_id], |r| r.get(0))?;
    let sold = asked == For::Page && c.query_row("SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id = ?1 AND side IS NOT 0)", [bot_id], |r| r.get(0))?;
    if other_type || sold { return Ok(Some(BEYOND_BUYS)); }
    match wash_sale_enabled {
        Some(true) => return Ok(Some("the wash-sale rule")),
        // The question's modal opens once the bot holds something, which only the metrics walk knows.
        None if c.query_row("SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id = ?1 AND status = 0)", [bot_id], |r| r.get(0))? => {
            return Ok(Some("the wash-sale question not answered yet"));
        }
        _ => {}
    }
    Ok(None)
}

/// The reason a bot with a sell, a swap or a corporate action in its history is refused.
pub const BEYOND_BUYS: &str = "orders other than scheduled buys";

fn object(text: &str, what: &str) -> Result<Map<String, Value>, WebError> {
    match serde_json::from_str(text) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(data(format!("{what} is not a JSON object"))),
    }
}

impl Bot {
    /// The user's bot with this id, deleted ones included (`current_user.bots.find`). Call `refusal` first, and `unrendered` on what this returns.
    /// The feed reads none of the four facts about the bot's orders, each of which may walk the whole history, and is not given them.
    pub fn find(c: &Connection, user_id: i64, id: i64, asked: For) -> Result<Option<Bot>, WebError> {
        let row = c.query_row(
            "SELECT id, type, status, label, exchange_id, settings, transient_data, started_at, stop_message_key FROM bots WHERE id = ?1 AND user_id = ?2", [id, user_id],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<i64>>(4)?,
                    r.get::<_, String>(5)?, r.get::<_, String>(6)?, r.get::<_, Option<String>>(7)?, r.get::<_, Option<String>>(8)?))).optional()?;
        let Some((id, class, status, label, exchange_id, settings, transient, started_at, stop_message_key)) = row else { return Ok(None) };
        let kind = if class.as_deref() == Some("Bots::DcaIndex") { Kind::Index } else { Kind::Basket };
        let status = BotStatus::from_i64(status).ok_or_else(|| data(format!("bot {id}: status {status}")))?;
        let (settings, transient) = (object(&settings, "bots.settings")?, object(&transient, "bots.transient_data")?);
        let exchange = c.query_row("SELECT id, name, type, maker_fee FROM exchanges WHERE id = ?1", [exchange_id], |r| {
            Ok(Exchange { id: r.get(0)?, name: r.get::<_, Option<String>>(1)?.unwrap_or_default(), class: r.get::<_, Option<String>>(2)?.unwrap_or_default(), maker_fee: r.get(3)? })
        }).optional()?.ok_or_else(|| data(format!("bot {id} has no exchange")))?;
        let started_at = started_at.map(|text| parse_time(&text).map_err(|e| data(format!("bot {id}: started_at {e:?}")))).transpose()?;
        let quote_asset_id = settings.get("quote_asset_id").and_then(Value::as_i64);
        let quote_asset = match quote_asset_id { Some(asset_id) => Asset::find(c, asset_id)?, None => None };

        let mut memberships = vec![];
        let mut statement = c.prepare("SELECT asset_id, ticker_id, target_allocation, in_index FROM bot_index_assets WHERE bot_id = ?1 ORDER BY id")?;
        let rows = statement.query_map([id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, Stored>(2)?.0, r.get::<_, Option<bool>>(3)?.unwrap_or(false)))
        })?.collect::<Result<Vec<_>, _>>()?;
        for (asset_id, ticker_id, target_allocation, in_index) in rows {
            let Some(asset) = Asset::find(c, asset_id)? else { continue };
            let ticker = Ticker::all(c, "t.id = ?1", &[&ticker_id])?.into_iter().next();
            memberships.push(Membership { asset, ticker, target_allocation, in_index });
        }

        let mut base_assets = vec![];
        if kind == Kind::Basket {
            for asset_id in allocation_ids(&settings) {
                if let Some(asset) = Asset::find(c, asset_id)? { base_assets.push(asset); }
            }
        }
        // set_tickers: available, trading-enabled tickers of this venue in the quote; a basket's are
        // further narrowed to every asset that is or ever was in its composition.
        let tradable = Ticker::all(c, "t.exchange_id = ?1 AND t.quote_asset_id IS ?2 AND t.available = 1 AND t.trading_enabled = 1", &[&exchange.id, &quote_asset_id])?;
        let tickers = match kind {
            Kind::Index => tradable,
            Kind::Basket => {
                let known: std::collections::HashSet<i64> = allocation_ids(&settings).into_iter().chain(memberships.iter().map(|m| m.asset.id)).collect();
                tradable.into_iter().filter(|ticker| known.contains(&ticker.base_asset_id)).collect()
            }
        };
        let api_key = c.query_row("SELECT status FROM api_keys WHERE user_id = ?1 AND exchange_id = ?2 AND key_type = 0 ORDER BY id LIMIT 1", [user_id, exchange.id], |r| r.get(0)).optional()?;
        let index = if kind == Kind::Index { index_row(c, &settings)? } else { None };
        let last_order = c.query_row("SELECT status, error_messages FROM transactions WHERE bot_id = ?1 ORDER BY id DESC LIMIT 1", [id], |r| {
            Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<String>>(1)?))
        }).optional()?.map(|(status, messages)| LastOrder {
            failed: status == Some(1),
            error_messages: messages.and_then(|text| serde_json::from_str::<Vec<Value>>(&text).ok()).unwrap_or_default().iter()
                .map(|m| m.as_str().map_or_else(|| m.to_string(), str::to_string)).collect(),
        });
        let exists = |condition: &str| -> Result<bool, WebError> {
            if asked == For::Feed { return Ok(false); }
            Ok(c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id = ?1{condition})"), [id], |r| r.get(0))?)
        };
        let mut bot = Bot {
            id, kind, status, label: label.unwrap_or_default(), exchange, settings, transient, started_at, stop_message_key, quote_asset, base_assets, memberships,
            tickers, api_key, index, last_order, has_orders: exists("")?, has_waiting_orders: exists(" AND status = 0 AND external_status IN (0, 1)")?,
            has_regular_waiting_orders: exists(" AND status = 0 AND external_status IN (0, 1) AND transaction_type = 'REGULAR'")?,
            has_submitted_orders: exists(" AND status = 0")?,
        };
        bot.fill_defaults();
        Ok(Some(bot))
    }

    /// What each concern's `after_initialize` gives a setting the row lacks, in memory, on every load
    /// (Bot::SmartIntervalable, LimitOrderable, QuoteAmountLimitable, PriceLimitable,
    /// PriceDropLimitable, MovingAverageLimitable, IndicatorLimitable, and
    /// Bots::DcaIndex::IndexAllocatable). Rails writes them with the next save; a page writes nothing,
    /// so a row from before a rule existed is rendered with these and stays as it is. `||=`: a
    /// setting that is absent, null or false takes its default. All of it is read from the database.
    fn fill_defaults(&mut self) {
        use serde_json::json;
        // The Smart Intervals amount first: it is computed from the row and the bot's tickers.
        let mut defaults: Vec<(&str, Value)> = vec![("smart_intervaled", json!(false))];
        if let Some(amount) = start::default_smart_interval_quote_amount(self) { defaults.push(("smart_interval_quote_amount", json!(amount))); }
        defaults.extend([("limit_ordered", json!(false)), ("limit_order_pcnt_distance", json!(0.001))]);
        match self.kind {
            Kind::Basket => {
                // `tickers.min_by { |t| t[:base] }&.id`: the member a condition watches until one is chosen.
                let first = self.tickers.iter().min_by(|a, b| a.base.as_bytes().cmp(b.base.as_bytes())).map(|ticker| ticker.id);
                defaults.extend([
                    ("quote_amount_limited", json!(false)), ("quote_amount_limit", json!(1000)),
                    ("price_limited", json!(false)), ("price_limit", json!(1_000_000)), ("price_limit_range_lower_bound", json!(0)),
                    ("price_limit_range_upper_bound", json!(1_000_000)), ("price_limit_timing_condition", json!("while")), ("price_limit_value_condition", json!("below")),
                    ("price_drop_limited", json!(false)), ("price_drop_limit", json!(0.2)), ("price_drop_limit_time_window_condition", json!("ath")),
                    ("moving_average_limited", json!(false)), ("moving_average_limit_timing_condition", json!("while")), ("moving_average_limit_value_condition", json!("below")),
                    ("moving_average_limit_in_ma_type", json!("sma")), ("moving_average_limit_in_timeframe", json!("one_day")), ("moving_average_limit_in_period", json!(9)),
                    ("indicator_limited", json!(false)), ("indicator_limit", json!(30)), ("indicator_limit_timing_condition", json!("while")),
                    ("indicator_limit_value_condition", json!("below")), ("indicator_limit_in_indicator", json!("rsi")), ("indicator_limit_in_timeframe", json!("one_day")),
                ]);
                if let Some(ticker_id) = first {
                    defaults.extend(["price_limit_in_ticker_id", "price_drop_limit_in_ticker_id", "moving_average_limit_in_ticker_id", "indicator_limit_in_ticker_id"].map(|key| (key, json!(ticker_id))));
                }
            }
            // `default_num_coins`: all of a bounded index, else ten.
            Kind::Index => defaults.extend([("num_coins", json!(self.bounded_universe_size().unwrap_or(10))), ("allocation_flattening", json!(0.0))]),
        }
        for (key, value) in defaults {
            if matches!(self.settings.get(key), None | Some(Value::Null | Value::Bool(false))) { self.settings.insert(key.to_string(), value); }
        }
    }

    /// What `refusal` cannot see in the row alone, because Rails' defaults are part of it: a span
    /// between two orders that the calendar here does not hold (`MAX_SPAN_SECONDS`), and for a
    /// working bot one too short to compute a checkpoint from (`MIN_SPAN_SECONDS`). Asked of every
    /// bot `find` returns, before anything is rendered from it.
    pub fn unrendered(&self) -> Option<&'static str> {
        let seconds = self.effective().map(|effective| effective.seconds());
        // Written so that a span that is not a number at all (a quotient of zeros) is refused too.
        if !seconds.is_some_and(|seconds| seconds <= MAX_SPAN_SECONDS) { return Some("Smart Intervals that leave more than a thousand years between two orders"); }
        if self.working() && !seconds.is_some_and(|seconds| seconds >= MIN_SPAN_SECONDS) { return Some("a working bot whose Smart Intervals leave no time between two orders"); }
        // Bot::QuoteAmountLimitable validates a cap that is switched on as no less than the smallest amount the quote states
        // (`minimum_quote_amount_limit`), and prints the error under the field; the tickers that say what that is are loaded by now.
        let floor = start::minimum_quote_amount_limit(self);
        if self.on("quote_amount_limited") && self.number("quote_amount_limit").is_some_and(|cap| cap.to_f() < floor) { return Some("a spending cap below the smallest amount its quote states"); }
        None
    }

    /// The ids of the user's bots that are not deleted, in the dashboard's order (`not_deleted.ordered`).
    pub fn ids(c: &Connection, user_id: i64) -> Result<Vec<i64>, WebError> {
        let mut statement = c.prepare("SELECT id FROM bots WHERE user_id = ?1 AND status != 3 ORDER BY position, id")?;
        let ids = statement.query_map([user_id], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
        Ok(ids)
    }

    /// Automation::DomIdable#dom_id: `tile_bots_dca_multi_asset_7`.
    pub fn dom_id(&self, prefix: &str) -> String {
        format!("{prefix}_{}_{}", self.param_key(), self.id)
    }

    /// `model_name.param_key`, the root of the form's field names.
    pub fn param_key(&self) -> &'static str {
        match self.kind { Kind::Basket => "bots_dca_multi_asset", Kind::Index => "bots_dca_index" }
    }

    pub fn working(&self) -> bool {
        BOT_WORKING.contains(&self.status)
    }

    pub fn setting(&self, key: &str) -> Option<&Value> {
        self.settings.get(key).filter(|value| !value.is_null())
    }

    pub fn number(&self, key: &str) -> Option<Num> {
        self.setting(key).and_then(Num::from_json)
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.setting(key).and_then(Value::as_str)
    }

    /// `settings[key] == true`: how the `…ed?` predicates of the rule concerns read their switch.
    pub fn on(&self, key: &str) -> bool {
        self.settings.get(key) == Some(&Value::Bool(true))
    }

    pub fn quote_symbol(&self) -> Option<&str> {
        self.quote_asset.as_ref().and_then(|asset| asset.symbol.as_deref())
    }

    pub fn interval(&self) -> Option<Interval> {
        self.text("interval").and_then(Interval::parse)
    }

    /// Bot::SmartIntervalable#effective_interval_duration, as the engine computes it.
    pub fn effective(&self) -> Option<Effective> {
        let interval = self.interval()?;
        let smart = if self.on("smart_intervaled") { self.number("smart_interval_quote_amount").map(|n| n.to_f()) } else { None };
        match (self.number("quote_amount"), smart) {
            (Some(quote), Some(smart)) => Some(schedule::effective(interval, quote.to_f(), Some(smart))),
            _ => Some(schedule::effective(interval, 0.0, None)),
        }
    }

    /// Bot::Startable#start_time_enabled?: ActiveModel's boolean cast of the stored value.
    pub fn start_time_enabled(&self) -> bool {
        cast_boolean(self.settings.get("start_time_enabled"))
    }

    /// Bot::Rebalanceable#rebalance_threshold: `value.presence&.to_d || 0.05`.
    pub fn rebalance_threshold(&self) -> Option<BigDec> {
        match self.settings.get("rebalance_threshold") {
            Some(Value::String(text)) if !text.trim().is_empty() => BigDec::parse(text).ok(),
            Some(number @ Value::Number(_)) => Num::from_json(number).and_then(|share| share.to_d()),
            _ => BigDec::parse("0.05").ok(),
        }
    }

    /// Bot::Startable#repeat_anchor_at: the stored start time while the starting-time rule is on, else `started_at`.
    pub fn anchor(&self) -> Option<DateTime<Utc>> {
        let stored = || self.text("start_at").filter(|text| !text.trim().is_empty()).and_then(|text| DateTime::parse_from_rfc3339(text).ok()).map(|time| time.with_timezone(&Utc));
        if self.start_time_enabled() { stored().or(self.started_at) } else { self.started_at }
    }

    /// `next_interval_checkpoint_at` and `last_interval_checkpoint_at`. Without an anchor Rails counts from now.
    pub fn checkpoints(&self, now: DateTime<Utc>) -> Option<Checkpoints> {
        let anchor = self.anchor().unwrap_or(now);
        Some(schedule::checkpoints(anchor.timestamp_micros(), now.timestamp_micros(), self.effective()?))
    }

    /// The same two checkpoints (next, last) before Rails rounds them: what it enqueues the next job for and measures the progress bar from.
    pub fn unrounded(&self, now: DateTime<Utc>) -> Option<(Unrounded, Unrounded)> {
        let anchor = self.anchor().unwrap_or(now);
        Some(schedule::unrounded(anchor.timestamp_micros(), now.timestamp_micros(), self.effective()?))
    }

    /// Automation::Schedulable#last_action_job_at: `Time.zone.parse` of the stored text.
    pub fn last_action_job_at(&self) -> Option<DateTime<Utc>> {
        let text = self.transient.get("last_action_job_at")?.as_str()?;
        DateTime::parse_from_rfc3339(text).ok().map(|time| time.with_timezone(&Utc))
    }

    /// Bot::Lifecycle#restarting?
    pub fn restarting(&self) -> bool {
        self.status == BotStatus::Stopped && self.transient.get("last_action_job_at").is_some_and(|value| !value.is_null() && value.as_str() != Some(""))
    }

    /// A basket's weights in stored order: (asset id, weight as a Float, as `allocation_for` reads it).
    pub fn allocations(&self) -> Vec<(i64, f64)> {
        let Some(Value::Object(weights)) = self.settings.get("allocations") else { return vec![] };
        weights.iter().filter_map(|(asset_id, stored)| Some((asset_id.parse().ok()?, weight(stored)?))).collect()
    }

    /// `allocations.values.sum(&:to_f)`: added in stored order, as a Float sum is.
    pub fn allocations_total(&self) -> f64 {
        self.allocations().iter().fold(0.0, |sum, (_, weight)| sum + weight)
    }

    /// Bots::DcaMultiAsset::Allocatable#allocations_balanced?: within 0.001 of one. An index bot has no such rule.
    pub fn allocations_balanced(&self) -> bool {
        self.kind != Kind::Basket || (self.allocations_total() - 1.0).abs() <= 0.001
    }

    pub fn excess_members(&self) -> usize {
        if self.kind == Kind::Basket { self.allocations().len().saturating_sub(MAX_ASSETS) } else { 0 }
    }

    pub fn one_asset(&self) -> bool {
        self.kind == Kind::Basket && self.allocations().len() == 1
    }

    /// The current members' tickers (Bot::Composition::Allocatable#composition_tickers), in membership order.
    pub fn composition_tickers(&self) -> Vec<&Ticker> {
        self.memberships.iter().filter(|m| m.in_index).filter_map(|m| m.ticker.as_ref()).collect()
    }

    /// `decimals[:quote]`: the least precise quote among the bot's tickers; a basket whose tickers
    /// are all gone asks its memberships instead.
    pub fn quote_decimals(&self) -> Option<u8> {
        let from_tickers = self.tickers.iter().map(|ticker| ticker.quote_decimals).min();
        match self.kind {
            Kind::Basket if self.tickers.is_empty() => self.composition_tickers().iter().map(|ticker| ticker.quote_decimals).min(),
            _ => from_tickers,
        }
    }

    pub fn api_key_correct(&self) -> bool {
        self.api_key == Some(ApiKeyStatus::Correct as i64)
    }

    /// Bots::DcaIndex#bounded_universe_size: how many members a Deltabadger index has.
    pub fn bounded_universe_size(&self) -> Option<i64> {
        let index = self.index.as_ref()?;
        (index.source.as_deref() == Some("deltabadger") && !index.top_coins.is_empty()).then_some(index.top_coins.len() as i64)
    }

    pub fn max_coins(&self) -> i64 {
        self.bounded_universe_size().unwrap_or(MAX_COINS)
    }

    /// Bots::DcaIndex#hold_all?
    fn hold_all(&self) -> bool {
        cast_boolean(self.settings.get("hold_all"))
    }

    /// Bots::DcaIndex#holds_whole_universe?
    pub fn holds_whole_universe(&self) -> bool {
        self.hold_all() && self.bounded_universe_size().unwrap_or(0) > 0
    }

    /// Bots::DcaIndex#effective_num_coins.
    pub fn effective_num_coins(&self) -> Option<i64> {
        if self.holds_whole_universe() { self.bounded_universe_size() } else { self.setting("num_coins").and_then(Value::as_i64) }
    }

    /// Bots::DcaIndex#display_index_name. `None` where it is the translated "Top coins" (no count and no name).
    pub fn display_index_name(&self) -> Option<String> {
        let category = self.text("index_category_id");
        if category == Some("nasdaq-100") { // COUNT_NAMED_INDICES
            if let Some(count) = self.effective_num_coins() {
                return Some(if self.holds_whole_universe() { "ND100".to_string() } else { format!("ND{count}") });
            }
        }
        if let Some(name) = self.text("index_name").filter(|name| !name.trim().is_empty()) { return Some(name.to_string()); }
        match self.text("index_type") {
            Some("top") | None | Some("") => self.setting("num_coins").map(|count| format!("Top {}", count.as_i64().map_or_else(|| count.to_string(), |n| n.to_string()))),
            // ponytail: `index_category_id.titleize` for a category index without a stored name; the wizard always stores one.
            _ => Some(category.map_or_else(|| "Index".to_string(), str::to_string)),
        }
    }
}

/// `render "svg/exchange-#{exchange.name_id}"`: the venue's mark (rust/templates/svg, rendered by Rails).
pub fn exchange_svg(name_id: &str) -> &'static str {
    match name_id {
        "alpaca" => include_str!("../../../templates/svg/_exchange_alpaca.html"),
        "binance" => include_str!("../../../templates/svg/_exchange_binance.html"),
        "binance_us" => include_str!("../../../templates/svg/_exchange_binance_us.html"),
        "bingx" => include_str!("../../../templates/svg/_exchange_bingx.html"),
        "bitget" => include_str!("../../../templates/svg/_exchange_bitget.html"),
        "bitmart" => include_str!("../../../templates/svg/_exchange_bitmart.html"),
        "bitrue" => include_str!("../../../templates/svg/_exchange_bitrue.html"),
        "bitvavo" => include_str!("../../../templates/svg/_exchange_bitvavo.html"),
        "bybit" => include_str!("../../../templates/svg/_exchange_bybit.html"),
        "coinbase" => include_str!("../../../templates/svg/_exchange_coinbase.html"),
        "gemini" => include_str!("../../../templates/svg/_exchange_gemini.html"),
        "hyperliquid" => include_str!("../../../templates/svg/_exchange_hyperliquid.html"),
        "ibkr" => include_str!("../../../templates/svg/_exchange_ibkr.html"),
        "kraken" => include_str!("../../../templates/svg/_exchange_kraken.html"),
        "kucoin" => include_str!("../../../templates/svg/_exchange_kucoin.html"),
        "mexc" => include_str!("../../../templates/svg/_exchange_mexc.html"),
        // Rails raises a missing-template error for a class without a mark; no exchange class is without one.
        _ => "",
    }
}

/// `allocations.keys.map(&:to_i)`, else the wizard's `base_asset_ids`.
fn allocation_ids(settings: &Map<String, Value>) -> Vec<i64> {
    match settings.get("allocations") {
        Some(Value::Object(weights)) if !weights.is_empty() => weights.keys().filter_map(|key| key.parse().ok()).collect(),
        _ => settings.get("base_asset_ids").and_then(Value::as_array).map(|ids| ids.iter().filter_map(Value::as_i64).collect()).unwrap_or_default(),
    }
}

/// Bots::DcaIndex#current_index.
fn index_row(c: &Connection, settings: &Map<String, Value>) -> Result<Option<IndexRow>, WebError> {
    let category = settings.get("index_category_id").and_then(Value::as_str).filter(|id| !id.is_empty());
    let by_category = settings.get("index_type").and_then(Value::as_str) == Some("category") && category.is_some();
    let read = |r: &rusqlite::Row<'_>| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?));
    let row = if by_category {
        c.query_row("SELECT source, top_coins, weights FROM indices WHERE external_id = ?1 ORDER BY id LIMIT 1", [category], read).optional()?
    } else {
        c.query_row("SELECT source, top_coins, weights FROM indices WHERE external_id = 'top-coins' AND source = 'internal' ORDER BY id LIMIT 1", [], read).optional()?
    };
    Ok(row.map(|(source, top_coins, weights)| IndexRow {
        source,
        top_coins: top_coins.and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok()).unwrap_or_default(),
        weights: weights.and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default(),
    }))
}

/// How ActiveModel reads an id out of a path: `String#to_i` (`"12abc"` is 12, `"1_0"` is 10, a
/// no-break space is not a space), and what is not positive, or is past the column's range, is no bot's id.
pub fn id_from_path(segment: &str) -> Option<i64> {
    i64::try_from(super::format::to_i(segment)).ok().filter(|id| *id > 0)
}
