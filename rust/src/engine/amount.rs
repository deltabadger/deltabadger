//! Bot::Accountable#pending_quote_amount, the per-leg minimum logic of Bot::OrderSetter#calculate_best_amount_info (Kraken's
//! and Alpaca's) and Bot::OrderCreator's rows. The basket split that decides each leg's quote is engine::basket::split.
use super::model::{Bot, Ticker};
use super::EngineError;
use crate::codec::format_time;
use crate::enums::{TxExternalStatus, TxStatus};
use crate::ruby::{to_sql, BigDec};
use super::venue_rules::{AmountLogic, MinimumLogic, Rounding, VenueRules, WireFormat};
use crate::venue::{NewOrder, OrderKind, Side};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

fn data(e: impl std::fmt::Debug) -> EngineError { EngineError::Data(format!("{e:?}")) }

pub use super::accounting::{pending_quote_amount, continue_runs_now, quote_amount_available_num,
    quote_amount_available, quote_amount_limit_reached, exact_cap_available, CapGuard, guard_exact_cap};

#[derive(Debug, Clone)]
pub struct OrderPlan {
    pub ticker: Ticker, pub side: Side, pub limit: bool, pub price: BigDec,
    /// order_amount_in_quote / price, unrounded (Transaction#before_save rounds it on write).
    pub amount: BigDec,
    pub quote_amount: BigDec,
    /// Volume denomination: Kraken `viqc`, or Alpaca quote sizing versus a guarded base quantity.
    pub quote_type: bool,
    /// What is sent: floored to quote_decimals (quote) or base_decimals (base).
    pub volume: BigDec,
    /// transactions.transaction_type the row is written with (Bot::OrderCreator#order_identity_values).
    pub kind: TxKind,
}

/// The transaction kinds the engine writes. A REBALANCE order is not a contribution: its rows stay out of the DCA carry, the
/// spend cap and the intent's pending-buy spending (Rails scopes them all to 'REGULAR').
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxKind { Regular, Rebalance }

impl TxKind {
    pub fn as_str(self) -> &'static str { match self { Self::Regular => "REGULAR", Self::Rebalance => "REBALANCE" } }
    pub fn parse(s: &str) -> Option<Self> { match s { "REGULAR" => Some(Self::Regular), "REBALANCE" => Some(Self::Rebalance), _ => None } }
}

#[derive(Debug)]
pub enum Sizing { Nothing, Ignored(OrderPlan), BelowMinimum(OrderPlan), Place(OrderPlan), ZeroPrice { decimals: i64 } }

/// Bot::OrderSetter#order_price for a buy: a limit buy goes below the last trade, put on the venue's tick
/// (Ticker#adjusted_price, floor); a market buy is the ask itself, unadjusted. The split values holdings at this price too.
/// `Err` only where Rails' adjusted_price raises (`VenueRules::adjusted_price`), which eligibility refuses before any tick.
pub fn order_price(bot: &Bot, ticker: &Ticker, reference: &BigDec, rules: &VenueRules) -> Result<BigDec, EngineError> { side_price(bot, ticker, reference, Side::Buy, rules) }

/// Bot::OrderSetter#order_price for either side: a limit sell goes ABOVE the last trade, `adjusted_price(reference × (1 + d))`
/// (a limit buy below it, × (1 - d)); a market order is the reference itself (the raw bid for a sell, the raw ask for a buy).
pub fn side_price(bot: &Bot, ticker: &Ticker, reference: &BigDec, side: Side, rules: &VenueRules) -> Result<BigDec, EngineError> {
    ticker.scales().map_err(EngineError::Data)?;
    let Some(d) = bot.limit_distance() else { return Ok(reference.clone()) };
    let multiplier = if side == Side::Sell { &BigDec::one() + &d } else { &BigDec::one() - &d };
    rules.adjusted_price(&(reference * &multiplier), ticker, Rounding::Floor).map_err(EngineError::Data)
}

/// set_orders for one sell order (Bot::Composition::OrderSetter#sell_order_data, then #calculate_best_amount_info): `amount`
/// is the base to sell (already `min(wanted, free)`), `price` the order price (`side_price`). Sells always size in base under
/// :base_and_quote_in_base, whatever the venue's buy table (order_setter.rb:29-31): the quote minimum in base rounded UP to
/// base_decimals, against the base floored DOWN. An exactly zero amount is `Ignored` (order_ignored) before the minimums; a
/// positive one that floors to zero is below the minimum (a skipped row).
pub fn size_sell(bot: &Bot, ticker: &Ticker, amount: &BigDec, price: &BigDec) -> Result<Sizing, EngineError> {
    let (base_decimals, _, _) = ticker.scales().map_err(EngineError::Data)?;
    let minimum = match ticker.minimum_quote_size.div(price) {
        Some(q) => q.ceil(base_decimals).max(ticker.minimum_base_size.clone()),
        None => return Ok(Sizing::ZeroPrice { decimals: ticker.price_decimals }),
    };
    let volume = amount.floor(base_decimals);
    let below = volume < minimum;
    let plan = OrderPlan { ticker: ticker.clone(), side: Side::Sell, limit: bot.limit_distance().is_some(), price: price.clone(),
                           amount: amount.clone(), quote_amount: amount * price, quote_type: false, volume, kind: TxKind::Regular };
    if amount.is_zero() { return Ok(Sizing::Ignored(plan)); }
    Ok(if below { Sizing::BelowMinimum(plan) } else { Sizing::Place(plan) })
}

/// `Err` only for a ticker precision outside `ruby::MAX_SCALE`, which eligibility refuses before any tick.
pub fn size(bot: &Bot, ticker: &Ticker, x: &BigDec, reference: &BigDec, rules: &VenueRules) -> Result<Sizing, EngineError> {
    if x.is_zero() { return Ok(Sizing::Nothing); }
    let (base_decimals, quote_decimals, _) = ticker.scales().map_err(EngineError::Data)?;
    let (distance, logic) = (bot.limit_distance(), rules.minimum_logic);
    let price = order_price(bot, ticker, reference, rules)?;
    let Some(amount) = x.div(&price) else { return Ok(Sizing::ZeroPrice { decimals: ticker.price_decimals }) };
    let rule = if distance.is_some() { logic.limit } else { logic.market };
    let (quote_type, below) = buy_minimum(ticker, &price, rule, &x.floor(quote_decimals), &amount.floor(base_decimals))?;
    let volume = if quote_type { x.floor(quote_decimals) } else { amount.floor(base_decimals) };
    let plan = OrderPlan { ticker: ticker.clone(), side: Side::Buy, limit: distance.is_some(), price, amount: amount.clone(), quote_amount: x.clone(), quote_type, volume, kind: TxKind::Regular };
    if amount.is_zero() { return Ok(Sizing::Ignored(plan)); } // set_orders checks this before the minimums
    Ok(if below { Sizing::BelowMinimum(plan) } else { Sizing::Place(plan) })
}

/// Bot::OrderSetter#calculate_best_amount_info's minimum for a buy at `price` (non-zero) under `rule`, from the quote `quote` and
/// the base `base` as they are compared (already floored): (sized in quote, below the minimum). `size` sizes with it, and the
/// exact-cap guard re-checks a reduced order with it in the order's own denomination, at the order's own price.
pub(crate) fn buy_minimum(ticker: &Ticker, price: &BigDec, rule: AmountLogic, quote: &BigDec, base: &BigDec) -> Result<(bool, bool), EngineError> {
    let (base_decimals, quote_decimals, _) = ticker.scales().map_err(EngineError::Data)?;
    Ok(match rule {
        // :quote. Only the quote amount floored to quote_decimals is compared, with minimum_quote_size; minimum_base_size is not
        // checked here; a limit qty under it is refused at placement (OrderPlan::limit_refusal).
        AmountLogic::Quote => (true, *quote < ticker.minimum_quote_size),
        AmountLogic::Base => (false, *base < ticker.minimum_base_size),
        // :base_and_quote_in_base: the quote minimum in base, rounded UP to base_decimals, against the base floored DOWN.
        AmountLogic::BaseAndQuoteInBase => {
            let minimum_quote_size_in_base = ticker.minimum_quote_size.div(price).expect("price is non-zero").ceil(base_decimals);
            (false, *base < minimum_quote_size_in_base.max(ticker.minimum_base_size.clone()))
        }
        AmountLogic::BaseOrQuote => {
            // :base_or_quote / :base_and_quote (Bot::OrderSetter#calculate_best_amount_info). Divisions are BigDecimal divisions.
            let minimum_base_size_in_quote = (&ticker.minimum_base_size * price).ceil(quote_decimals);
            let minimum_quote_amount = minimum_base_size_in_quote.max(ticker.minimum_quote_size.clone());
            let minimum_quote_amount_in_base = minimum_quote_amount.div(price).expect("price is non-zero");
            let minimum_quote_size_in_base = ticker.minimum_quote_size.div(price).expect("price is non-zero").ceil(base_decimals);
            let minimum_base_amount = minimum_quote_size_in_base.max(ticker.minimum_base_size.clone());
            let in_quote = minimum_quote_amount_in_base < minimum_base_amount;
            (in_quote, if in_quote { *quote < minimum_quote_amount } else { *base < minimum_base_amount })
        }
    })
}

/// Bot::OrderSetter#calculate_best_amount_info for a market order sized outside a DCA tick: a REBALANCE order
/// (Bot::Rebalancer#rebalance_order_data) or a signal bot's REGULAR one (Bots::Signal::OrderSetter#checked_amount_info), at
/// `price` (Exchange#market_price_for, already floored), `amount` base or `quote_amount` quote. A sell sizes in base under
/// :base_and_quote_in_base; a buy by the venue's market rule. Returns the plan (written with `kind`) and whether it is below
/// the minimum.
pub fn size_order(ticker: &Ticker, side: Side, price: &BigDec, amount: &BigDec, quote_amount: &BigDec, logic: MinimumLogic, kind: TxKind) -> Result<(OrderPlan, bool), EngineError> {
    let (base_decimals, quote_decimals, _) = ticker.scales().map_err(EngineError::Data)?;
    let zero = || EngineError::Data("a rebalance price of zero".into());
    let in_base = |x: &BigDec| -> Result<BigDec, EngineError> { Ok(x.div(price).ok_or_else(zero)?.ceil(base_decimals)) };
    let rule = if side == Side::Sell { AmountLogic::BaseAndQuoteInBase } else { logic.market };
    let (quote_type, minimum) = match rule {
        AmountLogic::Quote => (true, ticker.minimum_quote_size.clone()),
        AmountLogic::Base => (false, ticker.minimum_base_size.clone()),
        AmountLogic::BaseAndQuoteInBase => (false, in_base(&ticker.minimum_quote_size)?.max(ticker.minimum_base_size.clone())),
        AmountLogic::BaseOrQuote => {
            let minimum_quote_amount = (&ticker.minimum_base_size * price).ceil(quote_decimals).max(ticker.minimum_quote_size.clone());
            let minimum_base_amount = in_base(&ticker.minimum_quote_size)?.max(ticker.minimum_base_size.clone());
            let quote = minimum_quote_amount.div(price).ok_or_else(zero)? < minimum_base_amount;
            (quote, if quote { minimum_quote_amount } else { minimum_base_amount })
        }
    };
    let volume = if quote_type { quote_amount.floor(quote_decimals) } else { amount.floor(base_decimals) };
    let below = volume < minimum;
    Ok((OrderPlan { ticker: ticker.clone(), side, limit: false, price: price.clone(), amount: amount.clone(), quote_amount: quote_amount.clone(),
                    quote_type, volume, kind }, below))
}

/// Exchanges::Alpaca#wire_decimal: the value floored to `places` and spelled with exactly `places` fractional digits,
/// in decimal throughout. Never through Float, which can round a floored amount back up (812037646.065271585 at 9
/// places was sent as 812037646.065271616).
pub fn wire_decimal(d: &BigDec, places: u8) -> String {
    let s = d.floor(places).to_s_f();
    let (whole, fraction) = s.split_once('.').unwrap_or((s.as_str(), ""));
    if places == 0 { whole.to_string() } else { format!("{whole}.{fraction:0<width$}", width = usize::from(places)) }
}

impl OrderPlan {
    /// Exchange#limit_buy: a limit quantity (a quote volume floored, divided by the floored price, floored to base_decimals)
    /// that is zero or under minimum_base_size is refused before anything is sent, with Rails' message.
    pub fn limit_refusal(&self) -> Result<Option<String>, String> {
        if !self.limit { return Ok(None); }
        let t = &self.ticker;
        let (base_decimals, _, price_decimals) = t.scales()?;
        let price = self.price.floor(price_decimals);
        let qty = if self.quote_type {
            self.volume.div(&price).ok_or_else(|| "zero limit price".to_string())?.floor(base_decimals)
        } else { self.volume.floor(base_decimals) };
        Ok((qty.is_zero() || qty < t.minimum_base_size).then(|| format!(
            "Limit order of {} {} is below the minimum of {} {} for {}",
            qty.to_s_f(), t.base_code, t.minimum_base_size.to_s_f(), t.base_code, t.ticker)))
    }

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
                (OrderKind::Limit { price: wire_decimal(&price, price_decimals) }, wire_decimal(&qty, base_decimals), false)
            }
            // #set_market_order: a :quote amount is `notional` at quote_decimals; a :base one would be `qty`.
            WireFormat::Alpaca => (OrderKind::Market,
                wire_decimal(&self.volume, if self.quote_type { quote_decimals } else { base_decimals }), self.quote_type),
        };
        Ok(NewOrder { pair: t.ticker.clone(), side: self.side, kind, volume, quote_volume, cl_ord_id, deadline, day: !t.crypto })
    }
    /// Bots::Signal::OrderSetter#execute_api_order's `amount_info.merge(amount:, amount_type:)` where #venue_takes? (slice 8f):
    /// the caller's amount sent in its own denomination. `quote_type` and `volume` change together, the volume floored at that
    /// side's decimals as the venue's adapter floors it; the row's amounts stay what was sized.
    pub fn in_denomination(&self, amount: &BigDec, quote: bool) -> Result<OrderPlan, String> {
        let (base_decimals, quote_decimals, _) = self.ticker.scales()?;
        Ok(OrderPlan { quote_type: quote, volume: amount.floor(if quote { quote_decimals } else { base_decimals }), ..self.clone() })
    }
    /// Bot::OrderSetter#order_log_details: BigDecimals serialise as `to_s('F')` strings.
    pub fn log_details(&self) -> Value {
        json!({ "base": self.ticker.base_symbol, "quote": self.ticker.quote_symbol, "amount": self.amount.to_s_f(),
                "quote_amount": self.quote_amount.to_s_f(), "price": self.price.to_s_f() })
    }
}

pub enum RowKind { Submitted { external_id: String }, Failed { errors: Vec<String> }, Skipped }

/// Bot::OrderCreator: create_submitted_order! / create_failed_order! / create_skipped_order!.
pub fn write_order_row(c: &super::model::FencedTransaction<'_>, bot: &Bot, plan: &OrderPlan, kind: RowKind, created_at: DateTime<Utc>) -> Result<i64, EngineError> {
    write_order_row_in(c, bot, plan, kind, created_at)
}

/// Accepted orders are ledger facts, not a cache of the current credential's account state.
/// The intent already authorized dispatch; never discard the acknowledgement on a later fence.
pub fn write_accepted_order(c: &rusqlite::Transaction<'_>, bot: &Bot, plan: &OrderPlan, external_id: &str, created_at: DateTime<Utc>, run: Option<&super::model::RunIdentity>) -> Result<i64, EngineError> {
    let id = write_order_row_in(c, bot, plan, RowKind::Submitted { external_id: external_id.to_owned() }, created_at)?;
    c.execute("UPDATE transactions SET run_identity=?2 WHERE id=?1 AND run_identity IS NULL", params![id, run.map(|r| r.order_json().to_string())])?;
    Ok(id)
}

fn write_order_row_in(c: &Connection, bot: &Bot, plan: &OrderPlan, kind: RowKind, created_at: DateTime<Utc>) -> Result<i64, EngineError> {
    let zero = Some(0.0f64);
    let (status, external_status, external_id, errors, exec) = match kind {
        RowKind::Submitted { external_id } => (TxStatus::Submitted, Some(TxExternalStatus::Unknown as i64), Some(external_id), vec![], None),
        RowKind::Failed { errors } => (TxStatus::Failed, None, None, errors, zero),
        RowKind::Skipped => (TxStatus::Skipped, None, None, vec![], zero),
    };
    if let Some(ref id) = external_id {
        // An existing accepted row for this (exchange, external_id) wins.
        let existing: Option<i64> = c.query_row("SELECT id FROM transactions WHERE exchange_id = ?1 AND external_id = ?2",
            params![bot.exchange_id, id], |r| r.get(0)).optional()?;
        if let Some(existing) = existing { return Ok(existing); }
    }
    // A signal bot has no schedule: Bot::OrderCreator#order_identity_values leaves the two columns at their defaults.
    let bot_quote_amount = if bot.bot_type == super::model::SIGNAL_TYPE { BigDec::zero() } else { bot.quote_amount_num()?.to_dec().map_err(data)? };
    c.execute(
        "INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, amount, quote_amount, price, \
         amount_exec, quote_amount_exec, base, quote, base_asset_id, quote_asset_id, bot_interval, bot_quote_amount, transaction_type, error_messages, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?19, ?6, ?7, ?8, ?9, ?10, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?20, ?17, ?18, ?18)",
        params![
            bot.id, bot.exchange_id, external_id, status as i64, external_status, if plan.limit { 1 } else { 0 },
            to_sql(&plan.amount), to_sql(&plan.quote_amount), to_sql(&plan.price), exec,
            plan.ticker.base_symbol, plan.ticker.quote_symbol, plan.ticker.base_asset_id, plan.ticker.quote_asset_id,
            bot.interval().map(|i| i.as_str()).unwrap_or_default(), to_sql(&bot_quote_amount),
            serde_json::to_string(&errors).unwrap(), format_time(created_at), plan.side.code(), plan.kind.as_str(),
        ])?;
    Ok(c.last_insert_rowid())
}

/// Bots::Signal::OrderSetter#record_signal_failure before anything was sized (a price or balance read that failed, a check
/// that raised): create_failed_order! from `{ticker, side, order_type: :market_order}` alone, so no price or amounts; the
/// schedule columns at their defaults.
pub fn write_unsized_failure(c: &Connection, bot: &Bot, ticker: &Ticker, side: Side, errors: &[String], created_at: DateTime<Utc>) -> Result<i64, EngineError> {
    c.execute(
        "INSERT INTO transactions (bot_id, exchange_id, status, side, order_type, amount_exec, quote_amount_exec, base, quote, base_asset_id, quote_asset_id, \
         bot_interval, bot_quote_amount, transaction_type, error_messages, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 0, ?5, ?5, ?6, ?7, ?8, ?9, '', ?10, 'REGULAR', ?11, ?12, ?12)",
        params![bot.id, bot.exchange_id, TxStatus::Failed as i64, side.code(), 0.0f64, ticker.base_symbol, ticker.quote_symbol, ticker.base_asset_id, ticker.quote_asset_id,
                to_sql(&BigDec::zero()), serde_json::to_string(errors).unwrap(), format_time(created_at)])?;
    Ok(c.last_insert_rowid())
}

/// Bot::Startable#disable_starting_time!, which Bot::ActionJob runs after a clean run of a bot whose starting time is on
/// (action_job.rb:139). With settings.start_at equal to started_at (the only start-time state the engine runs, see
/// eligibility::start_time_reason) the schedule does not move, so Rails writes only the flag and updated_at
/// (update_columns): no carry capture, the carry window (settings_changed_at) stays where it was, and an order of the first
/// run still open counts later at what it fills. Rails' other branch (a start_at edited away from started_at captures the
/// carry as a settings change) is refused here rather than ported. No-op when the rule is off.
pub fn disable_starting_time(c: &Connection, bot_id: i64, now: DateTime<Utc>) -> Result<(), EngineError> {
    super::model::locked(c, |c| disable_locked(c, &super::model::load_bot(c, bot_id)?, now))
}

fn disable_locked(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<(), EngineError> {
    if !crate::web::bot::cast_boolean(bot.settings.get("start_time_enabled")) { return Ok(()); }
    let start_at = bot.settings.get("start_at").and_then(Value::as_str).filter(|s| !s.trim().is_empty());
    // An unreadable start_at refuses here, at its point of use; loading the bot never fails on it.
    let start_at_us = match start_at { Some(text) => Some(crate::codec::parse_time(text).map_err(data)?.timestamp_micros()), None => None };
    if start_at_us.is_some() && start_at_us != bot.started_at_us()? {
        return Err(EngineError::Data(format!("start_at {start_at:?} is not the bot's started_at")));
    }
    c.execute("UPDATE bots SET settings = json_set(settings, '$.start_time_enabled', json('false')), updated_at = ?2 WHERE id = ?1",
              params![bot.id, format_time(now)])?;
    Ok(())
}
