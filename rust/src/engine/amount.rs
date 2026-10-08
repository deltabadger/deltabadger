//! Bot::Accountable#pending_quote_amount, the per-leg minimum logic of Bot::OrderSetter#calculate_best_amount_info (Kraken's
//! and Alpaca's) and Bot::OrderCreator's rows. The basket split that decides each leg's quote is engine::basket::split.
use super::model::{Bot, Ticker};
use super::EngineError;
use crate::codec::format_time;
use crate::enums::{TxExternalStatus, TxStatus};
use crate::ruby::{to_sql, BigDec};
use super::venue_rules::{MinimumLogic, WireFormat};
use crate::venue::{NewOrder, OrderKind};
use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

fn data(e: impl std::fmt::Debug) -> EngineError { EngineError::Data(format!("{e:?}")) }

pub use super::accounting::{pending_quote_amount, continue_runs_now, quote_amount_available_num,
    quote_amount_available, quote_amount_limit_reached, exact_cap_available, CapGuard, guard_exact_cap};

#[derive(Debug, Clone)]
pub struct OrderPlan {
    pub ticker: Ticker, pub limit: bool, pub price: BigDec,
    /// order_amount_in_quote / price, unrounded (Transaction#before_save rounds it on write).
    pub amount: BigDec,
    pub quote_amount: BigDec,
    /// Volume denomination: Kraken `viqc`, or Alpaca quote sizing versus a guarded base quantity.
    pub quote_type: bool,
    /// What is sent: floored to quote_decimals (quote) or base_decimals (base).
    pub volume: BigDec,
}

#[derive(Debug)]
pub enum Sizing { Nothing, Ignored(OrderPlan), BelowMinimum(OrderPlan), Place(OrderPlan), ZeroPrice { decimals: i64 } }

/// Bot::OrderSetter#order_price for a buy: a limit buy goes below the last trade, floored to the pair's price decimals; a
/// market buy is the ask itself. The split values holdings at this price too. Only `Ticker#adjusted_price`'s default rule
/// (floor to price_decimals) is ported: a venue that overrides adjusted_price (Hyperliquid: 5 significant figures) is not
/// covered. `Err` only for a price precision outside `ruby::MAX_SCALE`, which eligibility refuses before any tick.
pub fn order_price(bot: &Bot, ticker: &Ticker, reference: &BigDec) -> Result<BigDec, EngineError> {
    let (_, _, price_decimals) = ticker.scales().map_err(EngineError::Data)?;
    Ok(match bot.limit_distance() {
        Some(d) => (reference * &(&BigDec::one() - &d)).floor(price_decimals),
        None => reference.clone(),
    })
}

/// `Err` only for a ticker precision outside `ruby::MAX_SCALE`, which eligibility refuses before any tick.
pub fn size(bot: &Bot, ticker: &Ticker, x: &BigDec, reference: &BigDec, logic: MinimumLogic) -> Result<Sizing, EngineError> {
    if x.is_zero() { return Ok(Sizing::Nothing); }
    let (base_decimals, quote_decimals, _) = ticker.scales().map_err(EngineError::Data)?;
    let distance = bot.limit_distance();
    let price = order_price(bot, ticker, reference)?;
    let Some(amount) = x.div(&price) else { return Ok(Sizing::ZeroPrice { decimals: ticker.price_decimals }) };
    let (quote_type, below) = match logic {
        // Exchanges::Alpaca: :quote. Only the quote amount floored to quote_decimals is compared, with minimum_quote_size;
        // minimum_base_size is never checked for a buy (a limit qty under it is sent, and Alpaca rejects it).
        MinimumLogic::Quote => (true, x.floor(quote_decimals) < ticker.minimum_quote_size),
        // Kraken minimum_amount_logic: anything but a market buy is :base.
        MinimumLogic::KrakenBaseOrQuote if distance.is_some() => (false, amount.floor(base_decimals) < ticker.minimum_base_size),
        MinimumLogic::KrakenBaseOrQuote => {
            // :base_or_quote (Bot::OrderSetter#calculate_best_amount_info). Divisions are BigDecimal divisions.
            let minimum_base_size_in_quote = (&ticker.minimum_base_size * &price).ceil(quote_decimals);
            let minimum_quote_amount = minimum_base_size_in_quote.max(ticker.minimum_quote_size.clone());
            let minimum_quote_amount_in_base = minimum_quote_amount.div(&price).expect("price is non-zero");
            let minimum_quote_size_in_base = ticker.minimum_quote_size.div(&price).expect("price is non-zero").ceil(base_decimals);
            let minimum_base_amount = minimum_quote_size_in_base.max(ticker.minimum_base_size.clone());
            let quote = minimum_quote_amount_in_base < minimum_base_amount;
            let below = if quote { x.floor(quote_decimals) < minimum_quote_amount } else { amount.floor(base_decimals) < minimum_base_amount };
            (quote, below)
        }
    };
    let volume = if quote_type { x.floor(quote_decimals) } else { amount.floor(base_decimals) };
    let plan = OrderPlan { ticker: ticker.clone(), limit: distance.is_some(), price, amount: amount.clone(), quote_amount: x.clone(), quote_type, volume };
    if amount.is_zero() { return Ok(Sizing::Ignored(plan)); } // set_orders checks this before the minimums
    Ok(if below { Sizing::BelowMinimum(plan) } else { Sizing::Place(plan) })
}

/// Kernel#format('%.Nf', BigDecimal) for a value ALREADY floored to `places`: the value goes through Float, then is
/// formatted at N places. Only for such input does this equal Ruby (0 differences in 32,000 cases; pinned by the
/// alpaca_sizing vectors). On unfloored input they differ: format("%.4f", BigDecimal("0.00265")) is "0.0026" in Ruby
/// 4.0.7, "0.0027" here. Callers floor first, as Exchanges::Alpaca does.
pub fn printf(d: &BigDec, places: u8) -> String {
    debug_assert!(d.floor(places) == *d, "printf needs a value already floored to {places} places");
    float_format(d, places)
}

/// The raw Float formatter behind `printf`, without the precondition. Exposed so a test can pin the unfloored difference.
pub fn float_format(d: &BigDec, places: u8) -> String { format!("{:.*}", usize::from(places), d.to_f()) }

/// Fixed decimal spelling after flooring: no binary conversion may undo the R1c order guard.
fn fixed_decimal(d: &BigDec, places: u8) -> Result<String, String> {
    let floored = d.floor(places);
    let legacy = printf(&floored, places);
    // Preserve Rails' representation when it does not increase the quantized amount.
    if BigDec::parse(&legacy).map_err(|e|format!("{e:?}"))? <= floored { return Ok(legacy); }
    let plain = floored.to_s_f();
    let (whole, fraction) = plain.split_once('.').unwrap_or((&plain, ""));
    Ok(if places == 0 { whole.to_string() } else { format!("{whole}.{fraction:0<width$}", width=usize::from(places)) })
}

impl OrderPlan {
    /// `Err` only for a ticker precision outside `ruby::MAX_SCALE` (then `size` refused the plan already).
    pub fn to_order(&self, cl_ord_id: String, deadline: DateTime<Utc>, wire: WireFormat) -> Result<NewOrder, String> {
        let t = &self.ticker;
        let (base_decimals, quote_decimals, price_decimals) = t.scales()?;
        let (kind, volume, quote_volume) = match wire {
            // Exchanges::Kraken#set_limit_order floors the price again; floor is idempotent.
            WireFormat::Kraken => (
                if self.limit { OrderKind::Limit { price: self.price.floor(price_decimals).to_s_f() } } else { OrderKind::Market },
                self.volume.to_s_f(), self.quote_type),
            // Exchanges::Alpaca#set_limit_order: qty = floor(quote, quote_decimals) / price, floored to base_decimals.
            WireFormat::Alpaca if self.limit => {
                let price = self.price.floor(price_decimals);
                let qty = if self.quote_type {
                    self.volume.div(&price).ok_or_else(||"zero limit price".to_string())?.floor(base_decimals)
                } else { self.volume.floor(base_decimals) };
                (OrderKind::Limit { price: fixed_decimal(&price, price_decimals)? }, fixed_decimal(&qty, base_decimals)?, false)
            }
            // #set_market_order: a :quote amount is `notional` at quote_decimals; a :base one would be `qty`.
            WireFormat::Alpaca => (OrderKind::Market,
                fixed_decimal(&self.volume, if self.quote_type { quote_decimals } else { base_decimals })?, self.quote_type),
        };
        Ok(NewOrder { pair: t.ticker.clone(), kind, volume, quote_volume, cl_ord_id, deadline, day: !t.crypto })
    }
    /// Bot::OrderSetter#order_log_details: BigDecimals serialise as `to_s('F')` strings.
    pub fn log_details(&self) -> Value {
        json!({ "base": self.ticker.base_symbol, "quote": self.ticker.quote_symbol, "amount": self.amount.to_s_f(),
                "quote_amount": self.quote_amount.to_s_f(), "price": self.price.to_s_f() })
    }
}

pub enum RowKind { Submitted { external_id: String }, Failed { errors: Vec<String> }, Skipped }

/// Bot::OrderCreator: persist_accepted_order! / create_failed_order! / create_skipped_order!.
pub fn write_order_row(c: &super::model::FencedTransaction<'_>, bot: &Bot, plan: &OrderPlan, kind: RowKind, created_at: DateTime<Utc>) -> Result<i64, EngineError> {
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
    let bot_quote_amount = bot.quote_amount_num()?.to_dec().map_err(data)?;
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

/// Bot::Startable#disable_starting_time!, which Bot::ActionJob runs after a clean run of a bot whose starting time is on
/// (action_job.rb:139): the rule turns off and, the settings having changed, Bot::Accountable captures the carry
/// (set_missed_quote_amount: pending_quote_amount, QuoteAmountLimitable's cap included) and its before_save caps it at
/// effective_quote_amount (`[missed_quote_amount.to_d, effective_quote_amount].min`, the first on a tie) and restarts the
/// window (settings_changed_at). ActiveRecord writes transient_data only when the hash differs by Ruby's `==` from the one
/// loaded: a carry numerically equal to the stored number, beside a present nil `missed_quote_amount_was_set`, leaves the
/// column (and the stored 0 rather than "0.0") as it was. No-op when the rule is off (`start_time_enabled?` is false).
pub fn disable_starting_time(c: &Connection, bot_id: i64, now: DateTime<Utc>) -> Result<(), EngineError> {
    super::model::locked(c, |c| disable_locked(c, &super::model::load_bot(c, bot_id)?, now))
}

fn disable_locked(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<(), EngineError> {
    if !crate::ruby::cast_boolean(bot.settings.get("start_time_enabled")) { return Ok(()); }
    let mut carry = pending_quote_amount(c, bot, now.timestamp_micros())?;
    if let Some(available) = quote_amount_available(c, bot)? { if available < carry { carry = available; } }
    let key = if bot.smart_quote_amount().is_some() { "smart_interval_quote_amount" } else { "quote_amount" };
    let raw = bot.settings.get(key).filter(|v| v.is_number()).ok_or_else(|| EngineError::Data(format!("{key} is not a number")))?;
    let effective = BigDec::from_f64(raw.as_f64().ok_or_else(|| EngineError::Data(format!("{key} {raw}")))?).map_err(data)?;
    let (value, stored) = if effective < carry { (effective, raw.clone()) } else { (carry.clone(), json!(carry.to_s_f())) };
    let old = |k: &str| bot.transient.get(k);
    let unchanged = old("missed_quote_amount").filter(|v| v.is_number())
        .and_then(|v| v.as_i64().map(BigDec::from_i64).or_else(|| v.as_f64().and_then(|f| BigDec::from_f64(f).ok())))
        .is_some_and(|stored| stored == value)
        && old("missed_quote_amount_was_set") == Some(&Value::Null);
    let at = format_time(now);
    if !unchanged {
        c.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.missed_quote_amount', json(?2), '$.missed_quote_amount_was_set', json('null')) \
                    WHERE id = ?1", params![bot.id, stored.to_string()])?;
    }
    c.execute("UPDATE bots SET settings = json_set(settings, '$.start_time_enabled', json('false')), settings_changed_at = ?2, updated_at = ?2 WHERE id = ?1",
              params![bot.id, at])?;
    Ok(())
}
