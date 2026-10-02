//! Bot::Accountable#pending_quote_amount, the per-leg minimum logic of Bot::OrderSetter#calculate_best_amount_info (Kraken's
//! and Alpaca's) and Bot::OrderCreator's rows. The basket split that decides each leg's quote is engine::basket::split.
use super::model::{Bot, Ticker};
use super::schedule::{checkpoints, effective, interval_count};
use super::EngineError;
use crate::codec::format_time;
use crate::enums::{BotStatus, TxExternalStatus, TxStatus};
use crate::ruby::{from_sql, to_sql, BigDec};
use super::venue_rules::{MinimumLogic, WireFormat};
use crate::venue::{NewOrder, OrderKind};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

fn at(us: i64) -> DateTime<Utc> { DateTime::from_timestamp_micros(us).expect("time in range") }
fn data(e: impl std::fmt::Debug) -> EngineError { EngineError::Data(format!("{e:?}")) }

/// Bot::Accountable#pending_quote_amount as Rails has it since its fill-credit fix (#448): what the bot owes since the
/// window opened, minus every submitted REGULAR buy in the window, read in ONE statement, so a row that changes state
/// between reads is counted once. Closed: its executed quote. Open or unknown: its submitted quote, or amount × price
/// (#invested_quote). Cancelled or abandoned: what it executed before it stopped (NULL reads 0); the rest is owed again.
/// Nothing in polling moves missed_quote_amount any more. The window is max(started_at, settings_changed_at), inclusive
/// for every row.
pub fn pending_quote_amount(c: &Connection, bot: &Bot, now_us: i64) -> Result<BigDec, EngineError> {
    if bot.status == BotStatus::Deleted { return Ok(BigDec::zero()); }
    let Some(started) = bot.started_at_us else { return Ok(BigDec::zero()) };
    let since = bot.calc_since_us().expect("started_at is set");
    // Rails binds a Time as its quoted_date text and SQLite compares text: bind the same text.
    let since_text = format_time(at(since));

    let mut invested = BigDec::zero();
    let mut s = c.prepare(
        "SELECT external_status, quote_amount, amount, price, quote_amount_exec FROM transactions WHERE bot_id = ?1 AND status = 0 AND side = 0 \
         AND transaction_type = 'REGULAR' AND external_status IN (0, 1, 2, 3, 4) AND created_at >= ?2")?;
    let mut rows = s.query(params![bot.id, since_text])?;
    while let Some(r) = rows.next()? {
        let dec = |i: usize| -> Result<Option<BigDec>, EngineError> { from_sql(r.get_ref(i)?).map_err(data) };
        let status: i64 = r.get(0)?;
        let row = if status == TxExternalStatus::Closed as i64 {
            dec(4)?.ok_or_else(|| EngineError::Data("closed order without quote_amount_exec".into()))?
        } else if status == TxExternalStatus::Unknown as i64 || status == TxExternalStatus::Open as i64 {
            match (dec(1)?, dec(2)?, dec(3)?) {
                (Some(q), _, _) => q,
                (None, Some(a), Some(p)) => &a * &p,
                _ => return Err(EngineError::Data("waiting order with neither quote_amount nor amount×price".into())),
            }
        } else {
            dec(4)?.unwrap_or_else(BigDec::zero) // cancelled or abandoned: `quote_amount_exec || 0`
        };
        invested = &invested + &row;
    }

    let interval = bot.interval().ok_or_else(|| EngineError::Data("interval".into()))?;
    let quote_amount = bot.quote_amount().ok_or_else(|| EngineError::Data("quote_amount".into()))?;
    let smart = bot.smart_quote_amount();
    let eff = effective(interval, quote_amount, smart);
    let intervals = interval_count(checkpoints(started, now_us, eff).last_us, since, eff);
    // Float × Integer is a Float; adding the BigDecimal carry coerces it with Float#to_d.
    let owed = BigDec::from_f64(smart.unwrap_or(quote_amount) * intervals as f64).map_err(data)?;
    Ok((&(&owed + &bot.missed_quote_amount()?) - &invested).max(BigDec::zero()))
}

#[derive(Debug, Clone)]
pub struct OrderPlan {
    pub ticker: Ticker, pub limit: bool, pub price: BigDec,
    /// order_amount_in_quote / price, unrounded (Transaction#before_save rounds it on write).
    pub amount: BigDec,
    pub quote_amount: BigDec,
    /// Kraken `viqc`: the volume is in quote.
    pub quote_type: bool,
    /// What is sent: floored to quote_decimals (quote) or base_decimals (base).
    pub volume: BigDec,
}

#[derive(Debug)]
pub enum Sizing { Nothing, Ignored(OrderPlan), BelowMinimum(OrderPlan), Place(OrderPlan), ZeroPrice { decimals: i64 } }

/// Bot::OrderSetter#order_price for a buy (Ticker#adjusted_price): a limit buy goes below the last trade, floored to the
/// pair's price decimals; a market buy is the ask itself. The split values holdings at this price too.
pub fn order_price(bot: &Bot, ticker: &Ticker, reference: &BigDec) -> BigDec {
    match bot.limit_distance() {
        Some(d) => (reference * &(&BigDec::one() - &d)).floor(ticker.price_decimals),
        None => reference.clone(),
    }
}

pub fn size(bot: &Bot, ticker: &Ticker, x: &BigDec, reference: &BigDec, logic: MinimumLogic) -> Sizing {
    if x.is_zero() { return Sizing::Nothing; }
    let distance = bot.limit_distance();
    let price = order_price(bot, ticker, reference);
    let Some(amount) = x.div(&price) else { return Sizing::ZeroPrice { decimals: ticker.price_decimals } };
    let (quote_type, below) = match logic {
        // Exchanges::Alpaca: :quote. Only the quote amount floored to quote_decimals is compared, with minimum_quote_size;
        // minimum_base_size is never checked for a buy (a limit qty under it is sent, and Alpaca rejects it).
        MinimumLogic::Quote => (true, x.floor(ticker.quote_decimals) < ticker.minimum_quote_size),
        // Kraken minimum_amount_logic: anything but a market buy is :base.
        MinimumLogic::KrakenBaseOrQuote if distance.is_some() => (false, amount.floor(ticker.base_decimals) < ticker.minimum_base_size),
        MinimumLogic::KrakenBaseOrQuote => {
            // :base_or_quote (Bot::OrderSetter#calculate_best_amount_info). Divisions are BigDecimal divisions.
            let minimum_base_size_in_quote = (&ticker.minimum_base_size * &price).ceil(ticker.quote_decimals);
            let minimum_quote_amount = minimum_base_size_in_quote.max(ticker.minimum_quote_size.clone());
            let minimum_quote_amount_in_base = minimum_quote_amount.div(&price).expect("price is non-zero");
            let minimum_quote_size_in_base = ticker.minimum_quote_size.div(&price).expect("price is non-zero").ceil(ticker.base_decimals);
            let minimum_base_amount = minimum_quote_size_in_base.max(ticker.minimum_base_size.clone());
            let quote = minimum_quote_amount_in_base < minimum_base_amount;
            let below = if quote { x.floor(ticker.quote_decimals) < minimum_quote_amount } else { amount.floor(ticker.base_decimals) < minimum_base_amount };
            (quote, below)
        }
    };
    let volume = if quote_type { x.floor(ticker.quote_decimals) } else { amount.floor(ticker.base_decimals) };
    let plan = OrderPlan { ticker: ticker.clone(), limit: distance.is_some(), price, amount: amount.clone(), quote_amount: x.clone(), quote_type, volume };
    if amount.is_zero() { return Sizing::Ignored(plan); } // set_orders checks this before the minimums
    if below { Sizing::BelowMinimum(plan) } else { Sizing::Place(plan) }
}

/// Kernel#format('%.Nf', BigDecimal) for a value ALREADY floored to `places`: the value goes through Float, then is
/// formatted at N places. Only for such input does this equal Ruby (0 differences in 32,000 cases; pinned by the
/// alpaca_sizing vectors). On unfloored input they differ: format("%.4f", BigDecimal("0.00265")) is "0.0026" in Ruby
/// 4.0.7, "0.0027" here. Callers floor first, as Exchanges::Alpaca does.
pub fn printf(d: &BigDec, places: i64) -> String {
    debug_assert!(d.floor(places) == *d, "printf needs a value already floored to {places} places");
    float_format(d, places)
}

/// The raw Float formatter behind `printf`, without the precondition. Exposed so a test can pin the unfloored difference.
pub fn float_format(d: &BigDec, places: i64) -> String { format!("{:.*}", places.max(0) as usize, d.to_f()) }

impl OrderPlan {
    pub fn to_order(&self, cl_ord_id: String, deadline: DateTime<Utc>, wire: WireFormat) -> NewOrder {
        let t = &self.ticker;
        let (kind, volume, quote_volume) = match wire {
            // Exchanges::Kraken#set_limit_order floors the price again; floor is idempotent.
            WireFormat::Kraken => (
                if self.limit { OrderKind::Limit { price: self.price.floor(t.price_decimals).to_s_f() } } else { OrderKind::Market },
                self.volume.to_s_f(), self.quote_type),
            // Exchanges::Alpaca#set_limit_order: qty = floor(quote, quote_decimals) / price, floored to base_decimals.
            WireFormat::Alpaca if self.limit => {
                let price = self.price.floor(t.price_decimals);
                let qty = self.volume.div(&price).map(|q| q.floor(t.base_decimals)).unwrap_or_else(BigDec::zero);
                (OrderKind::Limit { price: printf(&price, t.price_decimals) }, printf(&qty, t.base_decimals), false)
            }
            // #set_market_order: a :quote amount is `notional` at quote_decimals; a :base one would be `qty`.
            WireFormat::Alpaca => (OrderKind::Market,
                printf(&self.volume, if self.quote_type { t.quote_decimals } else { t.base_decimals }), self.quote_type),
        };
        NewOrder { pair: t.ticker.clone(), kind, volume, quote_volume, cl_ord_id, deadline }
    }
    /// Bot::OrderSetter#order_log_details: BigDecimals serialise as `to_s('F')` strings.
    pub fn log_details(&self) -> Value {
        json!({ "base": self.ticker.base_symbol, "quote": self.ticker.quote_symbol, "amount": self.amount.to_s_f(),
                "quote_amount": self.quote_amount.to_s_f(), "price": self.price.to_s_f() })
    }
}

pub enum RowKind { Submitted { external_id: String }, Failed { errors: Vec<String> }, Skipped }

/// Bot::OrderCreator: persist_accepted_order! / create_failed_order! / create_skipped_order!.
pub fn write_order_row(c: &Connection, bot: &Bot, plan: &OrderPlan, kind: RowKind, created_at: DateTime<Utc>) -> Result<i64, EngineError> {
    let zero = Some(0.0f64);
    let (status, external_status, external_id, errors, exec) = match kind {
        RowKind::Submitted { external_id } => (TxStatus::Submitted, Some(TxExternalStatus::Unknown as i64), Some(external_id), vec![], None),
        RowKind::Failed { errors } => (TxStatus::Failed, None, None, errors, zero),
        RowKind::Skipped => (TxStatus::Skipped, None, None, vec![], zero),
    };
    if let Some(ref id) = external_id {
        // persist_accepted_order!: an existing row for this (exchange, external_id) wins.
        let existing: Option<i64> = c.query_row("SELECT id FROM transactions WHERE exchange_id = ?1 AND external_id = ?2",
            params![bot.exchange_id, id], |r| r.get(0)).optional()?;
        if let Some(existing) = existing { return Ok(existing); }
    }
    let bot_quote_amount = BigDec::from_f64(bot.quote_amount().unwrap_or_default()).map_err(data)?;
    c.execute(
        "INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, amount, quote_amount, price, \
         amount_exec, quote_amount_exec, base, quote, base_asset_id, quote_asset_id, bot_interval, bot_quote_amount, transaction_type, error_messages, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8, ?9, ?10, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 'REGULAR', ?17, ?18, ?18)",
        params![
            bot.id, bot.exchange_id, external_id, status as i64, external_status, if plan.limit { 1 } else { 0 },
            to_sql(&plan.amount), to_sql(&plan.quote_amount), to_sql(&plan.price), exec,
            plan.ticker.base_symbol, plan.ticker.quote_symbol, plan.ticker.base_asset_id, plan.ticker.quote_asset_id,
            bot.interval().map(|i| i.as_str()).unwrap_or_default(), to_sql(&bot_quote_amount),
            serde_json::to_string(&errors).unwrap(), format_time(created_at),
        ])?;
    Ok(c.last_insert_rowid())
}
