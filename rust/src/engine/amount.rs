//! Bot::Accountable#pending_quote_amount, the per-leg minimum logic of Bot::OrderSetter#calculate_best_amount_info (Kraken's
//! and Alpaca's) and Bot::OrderCreator's rows. The basket split that decides each leg's quote is engine::basket::split.
use super::model::{Bot, Ticker};
use super::schedule::{checkpoints, effective, interval_count};
use super::EngineError;
use crate::codec::format_time;
use crate::enums::{BotStatus, TxExternalStatus, TxStatus};
use crate::ruby::{ruby_sum, to_sql, BigDec, Num};
use super::venue_rules::{MinimumLogic, WireFormat};
use crate::venue::{NewOrder, OrderKind};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

fn at(us: i64) -> Result<DateTime<Utc>, EngineError> { DateTime::from_timestamp_micros(us).ok_or_else(super::schedule::time_range_error) }
fn data(e: impl std::fmt::Debug) -> EngineError { EngineError::Data(format!("{e:?}")) }

/// Bot::Accountable#pending_quote_amount as Rails has it since its fill-credit fix (#448): what the bot owes since the
/// window opened, minus every submitted REGULAR buy in the window, read in ONE statement, so a row that changes state
/// between reads is counted once. Settled: its accepted normalized value. Open or unknown: its submitted quote, or amount × price
/// (#invested_quote). Cancelled or abandoned: normalized execution before it stopped; no execution credits zero.
/// Nothing in polling moves missed_quote_amount any more. The window is max(started_at, settings_changed_at), inclusive
/// for every row.
pub fn pending_quote_amount(c: &Connection, bot: &Bot, now_us: i64) -> Result<BigDec, EngineError> {
    if bot.status == BotStatus::Deleted { return Ok(BigDec::zero()); }
    let Some(started) = bot.started_at_us else { return Ok(BigDec::zero()) };
    let since = bot.calc_since_us().ok_or_else(super::schedule::time_range_error)?;
    // Rails binds a Time as its quoted_date text and SQLite compares text: bind the same text.
    let since_text = format_time(at(since)?);

    let mut invested = BigDec::zero();
    for row in crate::figures::fill::commitments(c,bot.id,&since_text).map_err(data)? {
        invested = invested.checked_add(&row.value).map_err(data)?;
    }

    let interval = bot.interval().ok_or_else(|| EngineError::Data("interval".into()))?;
    let quote_amount = bot.quote_amount().ok_or_else(|| EngineError::Data("quote_amount".into()))?;
    let smart = bot.smart_quote_amount();
    let eff = effective(interval, quote_amount, smart);
    let intervals = interval_count(checkpoints(started, now_us, eff)?.last_us, since, eff)?;
    // Float × Integer is a Float; adding the BigDecimal carry coerces it with Float#to_d.
    let owed = BigDec::from_f64(smart.unwrap_or(quote_amount) * intervals as f64).map_err(data)?;
    Ok((&(&owed + &bot.missed_quote_amount()?) - &invested).max(BigDec::zero()))
}

/// Bot::Lifecycle#start(start_fresh: false)'s `set_orders_now`, for a stopped bot the web continued. `use_delayed_first` is
/// always false there (only a fresh start computes a start time), so the bot runs at once unless #restarting_within_interval?:
/// it had ticked (last_action_job_at is set) and what it owes, capped as QuoteAmountLimitable caps it, is under one
/// `effective_quote_amount` (a smart split's amount when the split is on). Only the buy side: a selling bot is refused.
pub fn continue_runs_now(c: &Connection, bot: &Bot, now_us: i64) -> Result<bool, EngineError> {
    if bot.last_action_job_at_us()?.is_none() { return Ok(true); }
    let mut owed = pending_quote_amount(c, bot, now_us)?;
    if let Some(available) = quote_amount_available(c, bot)? { if available < owed { owed = available; } }
    let effective = bot.smart_quote_amount().or(bot.quote_amount()).ok_or_else(|| EngineError::Data("quote_amount".into()))?;
    Ok(owed >= BigDec::from_f64(effective).map_err(data)?)
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
                let qty = self.volume.div(&price).map(|q| q.floor(base_decimals)).unwrap_or_else(BigDec::zero);
                (OrderKind::Limit { price: printf(&price, price_decimals) }, printf(&qty, base_decimals), false)
            }
            // #set_market_order: a :quote amount is `notional` at quote_decimals; a :base one would be `qty`.
            WireFormat::Alpaca => (OrderKind::Market,
                printf(&self.volume, if self.quote_type { quote_decimals } else { base_decimals }), self.quote_type),
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

/// Bot::QuoteAmountLimitable#quote_amount_available_before_limit_reached (quote_amount_limitable.rb:50-79), with one stricter
/// term: an unresolved placement intent counts as spent at its full quote until it is settled.
/// None = no cap (the limit is off). Since the stamp (`created_at >= quote_amount_limit_enabled_at`; an unset stamp matches
/// nothing), submitted REGULAR buys count: closed at their executed quote, waiting at their submitted quote (or
/// amount × price), cancelled or abandoned at what they executed. Failed and skipped rows, sells and other types do not.
///
/// Computed with Ruby's own numerics: the closed and waiting buckets are BigDecimal (decimal columns); the
/// stopped bucket is `pluck(Arel.sql('COALESCE(quote_amount_exec, 0)'))`, SQLite's own INTEGER or REAL; each bucket is an
/// Array#sum in row order, the buckets are added closed + waiting + stopped, and the limit is the settings' Integer or Float.
/// So a Float leaks in where Rails' does: cap 60.03 with one cancelled fill of 60.02 leaves 0.00999999999999801.
pub fn quote_amount_available_num(c: &Connection, bot: &Bot) -> Result<Option<Num>, EngineError> {
    let Some(limit) = bot.quote_amount_limit().map_err(EngineError::Data)? else { return Ok(None) };
    let mut spent = Num::Int(0);
    if let Some(since) = bot.quote_amount_limit_enabled_at_us().map_err(EngineError::Data)? {
        // Rails binds a Time as its quoted_date text and SQLite compares text: bind the same text.
        let since_text = format_time(at(since)?);
        let mut closed = vec![];
        let mut waiting = vec![];
        let mut stopped = vec![];
        for row in crate::figures::fill::commitments(c,bot.id,&since_text).map_err(data)? {
            match row.status {
                2 => closed.push(row.cap_value),
                0 | 1 => waiting.push(row.cap_value),
                _ => stopped.push(row.cap_value),
            }
        }
        spent = ruby_sum(&closed).map_err(data)?.add(&ruby_sum(&waiting).map_err(data)?).map_err(data)?
            .add(&ruby_sum(&stopped).map_err(data)?).map_err(data)?;
    }
    if let Some(intent) = bot.rust_placement() {
        let quote = intent["quote_amount"].as_str().and_then(|q| BigDec::parse(q).ok()).ok_or_else(|| EngineError::Data(format!("rust_placement {intent}")))?;
        spent = spent.add(&Num::Dec(quote)).map_err(data)?;
    }
    let left = limit.sub(&spent).map_err(data)?;
    Ok(Some(if left.is_negative() { Num::Int(0) } else { left })) // [left, 0].max
}

/// `quote_amount_available_num` as the BigDecimal the engine sizes with (a Float as Float#to_d, which is what Rails' next
/// BigDecimal operation makes of it).
pub fn quote_amount_available(c: &Connection, bot: &Bot) -> Result<Option<BigDec>, EngineError> {
    quote_amount_available_num(c, bot)?.map(|n| n.to_dec().map_err(data)).transpose()
}

/// Bot::QuoteAmountLimitable#quote_amount_limit_reached?: what is left is under the pair's precision floor,
/// `1.0 / 10**min(quote_decimals)` over Bot#tickers (members and former members, available and trading-enabled; with none
/// the floor is 0 and nothing is under it). Not the venue minimum: a remainder between the two keeps the bot running.
pub fn quote_amount_limit_reached(c: &Connection, bot: &Bot) -> Result<bool, EngineError> {
    let Some(available) = quote_amount_available_num(c, bot)? else { return Ok(false) };
    let mut assets = bot.asset_ids();
    let mut s = c.prepare("SELECT asset_id FROM bot_index_assets WHERE bot_id = ?1")?;
    for a in s.query_map([bot.id], |r| r.get::<_, i64>(0))? { let a = a?; if !assets.contains(&a) { assets.push(a); } }
    let decimals: Option<i64> = c.query_row(
        "SELECT min(quote_decimals) FROM tickers WHERE exchange_id = ?1 AND quote_asset_id = ?2 AND available = 1 AND trading_enabled = 1 \
         AND base_asset_id IN (SELECT value FROM json_each(?3))",
        params![bot.exchange_id, bot.quote_asset_id(), serde_json::to_string(&assets).expect("ids serialise")], |r| r.get(0))?;
    let Some(d) = decimals else { return Ok(false) };
    // `1.0 / (10**d)`: a Float, compared as Ruby compares the remainder's own class with it.
    available.lt_f64(1.0 / 10f64.powi(d as i32)).map_err(data)
}
