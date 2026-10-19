//! What the engine may run Anything outside the slice is refused and named.
//! Refusal reads flags by Ruby TRUTHINESS (anything set), not by the `== true` the supported readers
//! use: a flag Rails might treat as on must never be quietly ignored.
use super::model::{self, Bot};
use super::placement;
use crate::crypto::Cipher;
use super::EngineError;
use crate::enums::{BotStatus, BOT_WORKING};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

/// `unreadable`: bots whose rows this build cannot read. Takeover refuses them; the running engine skips them. `notes`: what
/// `check` and the takeover report without refusing (a reference source with no stamp, staleness::Verdict::Unknown).
pub struct Report { pub eligible: Vec<i64>, pub problems: Vec<String>, pub unreadable: Vec<(i64, String)>, pub notes: Vec<String> }

const SUPPORTED_FLAGS: [&str; 3] = ["limit_ordered", "smart_intervaled", "quote_amount_limited"];
/// Bots::DcaMultiAsset::MAX_ASSETS.
pub const MAX_ASSETS: usize = 100;
/// Bots::DcaMultiAsset::Allocatable::ALLOCATION_TOLERANCE: Rails starts a manual basket only this close to 100 %.
const ALLOCATION_TOLERANCE: f64 = 0.001;
/// The reason a basket's sliders are off 100 %. It refuses a working bot, never a rebalance: a stopped bot's sliders may be
/// saved off 100 %, and Rails' DcaMultiAsset#rebalance_targets then reads no targets (nothing is due) unless a swap is
/// already owed its buy, which still lands.
pub const UNBALANCED: &str = "allocations: weights sum to";

/// Bots::DcaMultiAsset::Allocatable#allocations_balanced?: a market-cap basket does not own its weights; a manual one's
/// slider total (Array#sum) is within the tolerance of one. Not a basket: true.
pub fn allocations_balanced(bot: &Bot) -> bool {
    if bot.bot_type != "Bots::DcaMultiAsset" || bot.weighting() == "market_cap" { return true; }
    let weights: Vec<f64> = bot.allocations().unwrap_or_default().into_iter().map(|(_, w)| w).collect();
    (crate::ruby::float_sum(&weights) - 1.0).abs() <= ALLOCATION_TOLERANCE
}

/// bot_reasons, less the reason a rebalance does not care about (UNBALANCED): what refuses a rebalance's order.
pub fn rebalance_reasons(c: &Connection, bot: &Bot) -> Result<Vec<String>, EngineError> {
    Ok(bot_reasons(c, bot)?.into_iter().filter(|r| !r.starts_with(UNBALANCED)).collect())
}
/// One rule set with the page and the write path (web/bot/mod.rs, web/figure/holdings.rs): a selling batch is in flight too.
const PENDING_KEYS: [&str; 4] = ["rebalance_pending", "liquidation_pending", "liquidation_selling_since", "redeploy_pending"];

const UNKNOWN_STATUS: &str = "row(s) with a status Rails never writes";

fn set(v: &Value) -> bool { !matches!(v, Value::Null | Value::Bool(false)) && v != "false" && v != 0 && v != "" }

/// Work only Rails carries out: Bot::LiquidationState#liquidation_in_flight?, Bot::Composition::Redeployable#redeploy_in_flight?,
/// Bot::EvaluateRebalancersJob's candidates. Each rule covers the statuses Rails handles it for. The pending keys stay on
/// every status: Rails' automatic legs skip deleted/archived bots, but its halt resolutions (Bots::*ResolutionsController,
/// Bot::Resolve*Job) take any bot, and an archived bot's page offers them.
fn rails_work(c: &Connection, bot: &Bot) -> Result<Vec<String>, EngineError> {
    let mut r = vec![];
    // Transaction.statuses is submitted, failed, skipped. Any other status proves neither a fill nor a rejection: the order
    // may still be working at the venue, whatever this bot's own status.
    let unknown: i64 = c.query_row("SELECT count(*) FROM transactions WHERE bot_id = ?1 AND (status IS NULL OR status NOT IN (0, 1, 2))", [bot.id], |r| r.get(0))?;
    if unknown > 0 { r.push(format!("{unknown} {UNKNOWN_STATUS}")); }
    // Slice 6b, plan CB T4: threshold rebalancing runs here for a basket or index bot on every venue (rebalance.rs, run::run_rebalances). Its
    // pending swap is admitted in a phase this engine resumes (selling, buying), or halted ambiguous by this engine itself
    // (idle until the user resolves it after a handback); an ambiguous halt Rails wrote stays Rails'.
    let rebalances = rebalances_here(bot);
    // Deliberately broader than Rails' automatic jobs: a false refusal costs one check message, a takeover mid-liquidation costs money.
    for key in PENDING_KEYS {
        if bot.transient.get(key).is_none_or(Value::is_null) { continue; }
        if key == super::rebalance::PENDING_KEY && rebalances {
            match super::rebalance::pending(&bot.transient) {
                Ok(Some(p)) if [super::rebalance::PHASE_SELLING, super::rebalance::PHASE_BUYING].contains(&p.phase.as_str()) => continue,
                Ok(Some(p)) if p.phase == super::rebalance::PHASE_AMBIGUOUS && bot.transient.get(super::rebalance::RUST_HALT_KEY).is_some_and(set) => continue,
                Ok(None) => continue,
                Ok(Some(p)) => { r.push(format!("rebalance_pending in phase {:?}", p.phase)); continue; }
                Err(e) => { r.push(format!("{e:?}")); continue; }
            }
        }
        r.push(key.to_string());
    }
    // Bot::EvaluateRebalancersJob#candidates: REBALANCEABLE_TYPES, `.where.not(status: %i[deleted archived])`. A stopped bot still rebalances.
    let rebalanceable = matches!(bot.bot_type.as_str(), "Bots::DcaIndex" | "Bots::DcaMultiAsset") && !matches!(bot.status, BotStatus::Deleted | BotStatus::Archived);
    if rebalanceable && bot.settings.get("rebalance_enabled").is_some_and(set) {
        if let Err(e) = super::rebalance::threshold(&bot.settings) { r.push(format!("{e:?}")); }
    }
    let non_regular: i64 = c.query_row(
        "SELECT count(*) FROM transactions WHERE bot_id = ?1 AND transaction_type <> 'REGULAR' AND NOT (?2 AND transaction_type = 'REBALANCE') \
         AND status = 0 AND external_status IN (0, 1)",
        params![bot.id, rebalances], |r| r.get(0))?;
    if non_regular > 0 { r.push(format!("{non_regular} waiting LIQUIDATION/REDEPLOY/REBALANCE order(s)")); }
    // Bot::LiquidationState#unresolved_liquidation_orders: ids the user attested to (liquidation_resolved_orders) are accounted for.
    // Ruby's `to_i`; an id this cannot read counts as unresolved.
    let resolved: Vec<i64> = match bot.transient.get("liquidation_resolved_orders") {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_i64().or_else(|| v.as_str()?.trim().parse().ok())).collect(),
        _ => vec![],
    };
    let mut s = c.prepare("SELECT id FROM transactions WHERE bot_id = ?1 AND transaction_type = 'LIQUIDATION' AND external_status = 4")?;
    let abandoned = s.query_map([bot.id], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?.into_iter().filter(|id| !resolved.contains(id)).count();
    if abandoned > 0 { r.push(format!("{abandoned} unresolved abandoned LIQUIDATION order(s)")); }
    // An abandoned REDEPLOY (RULING-B2B-1 MQ1) or REBALANCE leg (RULING-B2B-R1 item 1) has an unknown fill and Rails trades
    // on: its spent counts 0, so a Yes could buy the proceeds twice, and a rebalance's flight cash may already be the asset.
    // Rails has no way to account for either, so the refusal stands until the row itself is settled.
    for (kind, label) in [("REDEPLOY", "unresolved abandoned REDEPLOY"), ("REBALANCE", "abandoned REBALANCE")] {
        let n: i64 = c.query_row("SELECT count(*) FROM transactions WHERE bot_id = ?1 AND transaction_type = ?2 AND external_status = 4",
                                 params![bot.id, kind], |r| r.get(0))?;
        if n > 0 { r.push(format!("{n} {label} order(s) (fill unknown)")); }
    }
    Ok(r)
}

/// Whether this engine rebalances the bot: a basket or index bot (Bot::EvaluateRebalancersJob's REBALANCEABLE_TYPES). Bot::Rebalancer
/// names no venue, and `admission` refuses every venue this engine does not run (plan CB T4).
fn rebalances_here(bot: &Bot) -> bool { matches!(bot.bot_type.as_str(), "Bots::DcaIndex" | "Bots::DcaMultiAsset") }

pub fn bot_reasons(c: &Connection, bot: &Bot) -> Result<Vec<String>, EngineError> {
    bot_reasons_with(c, bot, &mut SplitRows::new())
}

/// The split rows of one user on one venue (base_asset_id, base_currency), read once per check: a guarded write must stay
/// short, and the ledger holds every activity of the account with no index on its entry type.
type SplitRows = std::collections::HashMap<(i64, i64), Vec<(Option<i64>, String)>>;

fn bot_reasons_with(c: &Connection, bot: &Bot, splits: &mut SplitRows) -> Result<Vec<String>, EngineError> {
    bot.validate_times()?;
    model::merged_history_cutoff(c, bot)?;
    let mut r = rails_work(c, bot)?;
    // Nothing else in such a history can be read (the shared row reader refuses it): name it and stop.
    if r.iter().any(|reason| reason.ends_with(UNKNOWN_STATUS)) { return Ok(r); }
    let index = bot.bot_type == "Bots::DcaIndex";
    if !index && bot.bot_type != "Bots::DcaMultiAsset" { r.push(format!("type {} (only DCA baskets and index bots)", bot.bot_type)); }
    let exchange: Option<String> = c.query_row("SELECT type FROM exchanges WHERE id = ?1", [bot.exchange_id], |r| r.get(0)).optional()?;
    // Kraken, Binance and KuCoin (plan K ruling O6): buying baskets (P1-T0 reverses spec E2's owner question 4) without an amount limit.
    let kraken = match exchange.as_deref() { Some("Exchanges::Kraken") => Some("Kraken"), Some("Exchanges::Binance") => Some("Binance"),
                                             Some("Exchanges::Kucoin") => Some("KuCoin"), _ => None };
    let alpaca = exchange.as_deref() == Some("Exchanges::Alpaca");
    let members = if index { index_reasons(c, bot, alpaca, &mut r)? } else { composition_reasons(bot, &mut r) };
    let selling = bot.settings.get("direction").is_some_and(|v| v == "selling");
    if let Some(why) = admission(exchange.as_deref(), index, selling) { r.push(why.into()); }
    match bot.settings.get("direction") {
        None | Some(Value::Null) => {}, Some(v) if v == "buying" => {},
        Some(v) if v == "selling" => sell_reasons(c, bot, alpaca, &mut r)?,
        Some(d) => r.push(format!("direction {d}")),
    }
    if let Some(obj) = bot.settings.as_object() {
        for (k, v) in obj {
            let flag = k.ends_with("_limited") || k.ends_with("_ordered") || k.ends_with("_intervaled");
            if !flag || !set(v) || SUPPORTED_FLAGS.contains(&k.as_str()) { continue; }
            match k.as_str() {
                "price_limited" | "sell_price_limited" => r.extend(price_rule_reason(bot, k, v)),
                _ => r.push(k.clone()),
            }
        }
    }
    if bot.settings.get("start_time_enabled").is_some_and(set) && BOT_WORKING.contains(&bot.status) { r.extend(start_time_reason(c, bot)?); }
    if bot.settings.get("smart_intervaled").is_some_and(set) && !bot.smart_quote_amount().is_some_and(|a| a > 0.0) {
        r.push("smart interval amount missing, not a JSON number, or not positive".into());
    }
    if bot.limit_ordered() && bot.limit_distance().is_none() { r.push("limit_order_pcnt_distance is not a number".into()); }
    if BOT_WORKING.contains(&bot.status) && bot.started_at_us()?.is_none() { r.push("started_at missing (never ticks)".into()); }
    if bot.interval().is_none() { r.push("interval".into()); }
    if !bot.quote_amount().is_some_and(|q| q > 0.0) { r.push("quote_amount".into()); }
    if bot.restatement_generation > 0 && model::all_crypto(c, bot)? { r.push("restated prices".into()); }
    let wash: Option<Option<bool>> = c.query_row("SELECT wash_sale_enabled FROM users WHERE id = ?1", [bot.user_id], |r| r.get(0)).optional()?;
    match wash { None => r.push("user not found".into()), Some(Some(true)) => r.push("wash_sale enabled for the user".into()), _ => {} }
    if bot.quote_amount_limited() {
        if let Some(venue) = kraken { r.push(format!("quote_amount_limited ({venue}: amount limits are not supported by this engine yet)")); }
        if let Err(e) = bot.quote_amount_limit() { r.push(e); }
        if let Err(e) = bot.quote_amount_limit_enabled_at_us() { r.push(e); }
    }
    if !alpaca && bot.merged_history() { r.push("merged history (merged_history_until_id)".into()); }
    history_reasons(c, bot, alpaca, &members, splits, &mut r)?;
    member_reasons(c, bot, alpaca, &members, &mut r)?;
    // The walk must read; every recorded split applied (the last instant the figures represent), so no clock is read here.
    let sold: bool = c.query_row("SELECT EXISTS (SELECT 1 FROM transactions WHERE bot_id = ?1 AND side = 1)", [bot.id], |r| r.get(0))?;
    if (alpaca || sold) && r.is_empty() {
        if let Err(e) = super::basket::walk(c, bot, DateTime::from_timestamp_nanos(i64::MAX)) {
            // A swap whose pending sell or buy closed before its figures are known: admitted awaiting them. The DCA tick stands
            // down while it is pending; the rebalance fire re-reads that row before anything else is decided.
            if !super::rebalance::awaiting_figures(c, bot)? { return Err(e); }
            return Ok(r);
        }
    }
    if alpaca && r.is_empty() { super::accounting::validate_stored_amounts(&bot.settings, &bot.transient)?; super::accounting::quote_amount_available_num(c, bot)?; }
    Ok(r)
}

/// A price rule (slice 7a) runs with the `pause` action and values Rails compares; its flip actions are slice 7c. A flag
/// set to anything but `true` (which Rails reads as off) is refused, as every flag outside the slice.
fn price_rule_reason(bot: &Bot, key: &str, value: &Value) -> Option<String> {
    if value != &Value::Bool(true) { return Some(key.to_string()); }
    match bot.price_rule(key == "sell_price_limited") {
        Err(e) => Some(e),
        Ok(Some(rule)) if rule.action != "pause" => Some(format!("{key} with the {} action (the flip actions come with slice 7c)", rule.action)),
        Ok(_) => None,
    }
}

/// Which bots this build runs, by venue, class and member count: the one rule the engine's guard and the bot pages
/// (web::bot::refusal) both apply, so a page is never served for a bot the engine refuses. Kraken, Binance and KuCoin take
/// buying baskets and index bots of any size (P1-T0, plan CI, plan K O6); an index bot never sells (Bots::DcaIndex is not
/// Bot::Reversible); a basket of any size sells on every venue (6a, 6b, plan CB T3: the ranked DCA-out is venue-neutral).
/// What else a bot needs (its amount limit on Kraken/Binance, market-cap weights) is each side's own named reason.
pub fn admission(exchange: Option<&str>, index: bool, selling: bool) -> Option<&'static str> {
    match exchange {
        Some("Exchanges::Alpaca") => {}
        Some("Exchanges::Kraken" | "Exchanges::Binance" | "Exchanges::Kucoin") => {}
        _ => return Some("a bot on an exchange other than Alpaca, Kraken, Binance and KuCoin"),
    }
    if selling && index { Some("an index bot that sells") } else { None }
}


/// Selling: a one-asset basket on Kraken, Binance or Alpaca (slice 6a, one_asset_sell_orders_data), or a wider Alpaca basket
/// (6b, the ranked DCA-out of get_sell_orders_data), sized from the sell sentence and scheduled on the sell cadence. Refused
/// (beyond `admission`): rows on another venue outside Alpaca (there live_free_balance caps a sale at this venue's own
/// units), and a sell cadence no schedule here can hold.
fn sell_reasons(c: &Connection, bot: &Bot, alpaca: bool, r: &mut Vec<String>) -> Result<(), EngineError> {
    let foreign: i64 = c.query_row("SELECT count(*) FROM transactions WHERE bot_id = ?1 AND status = 0 AND exchange_id != ?2", params![bot.id, bot.exchange_id], |r| r.get(0))?;
    if foreign > 0 && !alpaca { r.push(format!("selling with {foreign} order(s) placed on another venue")); }
    if let Some(v) = bot.settings.get("sell_interval").filter(|v| !v.is_null() && v.as_str().is_none_or(|s| !s.trim().is_empty())) {
        if v.as_str().and_then(super::schedule::Interval::parse).is_none() { r.push(format!("sell_interval {v}")); }
    }
    for key in ["sell_amount", "sell_quote_amount", "smart_interval_base_amount"] {
        if let Some(v) = bot.settings.get(key).filter(|v| !v.is_null()) {
            if !(v.is_number() || v.as_str().is_some_and(|s| s.trim().is_empty() || crate::ruby::BigDec::parse(s.trim()).is_ok())) { r.push(format!("{key} is not a number")); }
            if super::schedule::to_d(v).is_some_and(|d| d < crate::ruby::BigDec::zero()) { r.push(format!("{key} is negative")); }
        }
    }
    // The span between two sells, as the web bounds a working bot's (MIN_SPAN_SECONDS, MAX_SPAN_SECONDS).
    let seconds = bot.effective().map(|e| e.seconds());
    if !seconds.is_some_and(|s| (crate::web::bot::MIN_SPAN_SECONDS..=crate::web::bot::MAX_SPAN_SECONDS).contains(&s)) {
        r.push("a sell cadence under a second or over a thousand years".into());
    }
    Ok(())
}

/// A pending starting time (Bot::Startable) runs only in the state Bot::Lifecycle#start(start_fresh: true) leaves it:
/// settings.start_at equal to started_at, so Startable#repeat_anchor_at and the started_at the engine schedules from are one
/// time, and no continue waiting. Rails' continue (start_fresh: false) ignores the starting time but still anchors the grid
/// on start_at: it can buy at once and count every interval up to a start_at that was moved later; a start_at edited on a
/// running bot moves the grid the same way. Read by truthiness, like the flags: anything Rails might treat as on.
fn start_time_reason(c: &Connection, bot: &Bot) -> Result<Option<String>, EngineError> {
    // An unreadable start_at refuses the bot as unreadable, here at its point of use, never at load. A blank one is absent,
    // as Rails' `presence` reads it.
    let start_at = match bot.settings.get("start_at").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
        None => None,
        Some(text) => Some(crate::codec::parse_time(text).map_err(|e| EngineError::Data(format!("start_at: {e:?}")))?.timestamp_micros()),
    };
    let started = bot.started_at_us()?;
    // No start_at: Rails' repeat_anchor_at falls back to started_at, so the rule moves nothing, and the next clean run turns
    // it off with the flag alone (disable_starting_time!'s nil-anchor branch, amount::disable_starting_time). A missing
    // started_at is refused on its own ("never ticks").
    // A continue with no start_at is an ordinary continue: Lifecycle#start(start_fresh: false) leaves started_at alone, and
    // the grid it continues on is started_at's.
    if start_at.is_none() { return Ok(None); }
    if bot.transient.get("rust_continue_start").is_some() {
        return Ok(Some("start_time_enabled with a continue (Rails ignores the starting time on a continue and may buy at once)".into()));
    }
    let Some(at) = start_at.filter(|at| Some(*at) == started) else {
        return Ok(Some("start_time_enabled with a start_at that is not the bot's started_at (only a fresh start's starting time is supported)".into()));
    };
    // Every day or every week at a time: the stored instant must read as that wall time in the owner's zone. Rails' fixed-day
    // step across a daylight-saving change can store it an hour off, earlier or later, with both anchors still equal.
    let mode = bot.settings.get("start_time_mode").and_then(Value::as_str);
    // Every mode, date included, needs a zone this build reads as Rails does.
    let name: String = c.query_row("SELECT time_zone FROM users WHERE id = ?1", [bot.user_id], |r| r.get(0))?;
    let Some(zone) = crate::web::timezone::strict(&name) else { return Ok(Some(format!("start_time_enabled in an unknown time zone {name:?}"))) };
    if mode == Some("date") { return Ok(None); }
    if !mode.is_some_and(|m| crate::web::bot::start::MODES.contains(&m)) {
        return Ok(Some(format!("start_time_enabled with start_time_mode {mode:?}")));
    }
    let local = DateTime::from_timestamp_micros(at).ok_or_else(|| EngineError::Data(format!("start_at {at} is out of range")))?.with_timezone(&zone);
    let chosen = crate::web::bot::start::parse_hhmm(bot.settings.get("start_time_of_day").and_then(Value::as_str));
    use chrono::Timelike;
    if chosen.is_ok_and(|(h, m)| (local.hour(), local.minute(), local.second(), local.nanosecond()) == (h, m, 0, 0)) {
        // The chosen wall time must also occur exactly once that day: at a repeated time, which occurrence Rails picks is
        // not proven, so a row left at either one is refused, as the web start refuses it.
        return Ok(match crate::web::timezone::resolve_local(zone, local.naive_local()) {
            Ok((_, only)) if only == local => None,
            Ok(_) => Some(format!("start_time_enabled with a start_at that is not the one instant of {} in {name}", local.format("%H:%M"))),
            Err(reason) => Some(format!("start_time_enabled: {reason}")),
        });
    }
    Ok(Some(format!("start_time_enabled with a start_at at {} in {name}, not the chosen time {:?} (a daylight-saving change moved it)",
                    local.format("%H:%M:%S"), bot.settings.get("start_time_of_day"))))
}

/// settings.allocations: what Rails would start, within what this build ports (manual weights, 1..MAX_ASSETS members on
/// every venue). Returns the member asset ids in settings order.
fn composition_reasons(bot: &Bot, r: &mut Vec<String>) -> Vec<i64> {
    if bot.weighting() != "manual" { r.push(format!("weighting {} (market-cap weights are not supported by this engine yet)", bot.weighting())); }
    let Some(a) = bot.allocations() else {
        r.push("allocations: an asset id or a weight this build does not read".into());
        return vec![];
    };
    if a.is_empty() { r.push("allocations: none".into()); }
    if a.len() > MAX_ASSETS { r.push(format!("allocations: {} assets (at most {MAX_ASSETS})", a.len())); }
    // Bots::DcaMultiAsset::Allocatable#allocations_balanced?.
    if a.iter().any(|(_, w)| *w < 0.0) { r.push("allocations: a negative weight".into()); }
    if !allocations_balanced(bot) { r.push(format!("{UNBALANCED} {}, not 1", crate::ruby::float_sum(&a.iter().map(|(_, w)| *w).collect::<Vec<_>>()))); }
    // Keys that name one id ("7", "07") would make the basket carry one asset as two members.
    let ids: Vec<i64> = a.into_iter().map(|(id, _)| id).collect();
    for (i, id) in ids.iter().enumerate() {
        if ids[..i].contains(id) && !ids[i + 1..].contains(id) { r.push(format!("allocations: asset {id} listed more than once")); }
    }
    ids
}

/// Alpaca REGULAR buys/sells and merged rows are covered by the normalized walk, and an Alpaca index bot's settled
/// REBALANCE/LIQUIDATION/REDEPLOY rows too (B2b). Other histories retain their named refusals. Stock split matching and
/// trust checks remain in engine::splits.
fn history_reasons(c: &Connection, bot: &Bot, alpaca: bool, members: &[i64], splits: &mut SplitRows, r: &mut Vec<String>) -> Result<(), EngineError> {
    crate::figures::fill::validate_row_times(c,bot.id).map_err(|e|EngineError::Data(format!("{e:?}")))?;
    order_kind_reasons(c, bot, alpaca, r)?;
    if !model::all_crypto(c, bot)? { return Ok(()); }
    // Bot::Restatable#grouped_split_rows applies a split recorded in account_transactions (corporate_action 'split') inside
    // the walk; the ported walk does not. Crypto never splits, so this refuses only what this build does not port. Broader than Rails'
    // match (any member's asset id, symbol or venue spelling), so it can only refuse more.
    let rows = match splits.entry((bot.user_id, bot.exchange_id)) {
        std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
        std::collections::hash_map::Entry::Vacant(e) => {
            let mut s = c.prepare(
                "SELECT base_asset_id, base_currency FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND entry_type = 15 \
                 AND (CASE WHEN json_valid(raw_data) THEN json_extract(raw_data, '$.corporate_action') END) = 'split'")?;
            let found = s.query_map(params![bot.user_id, bot.exchange_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?;
            e.insert(found)
        }
    };
    let ids = serde_json::to_string(members).expect("ids serialise");
    let mut s = c.prepare("SELECT symbol FROM assets WHERE id IN (SELECT value FROM json_each(?2)) \
                           UNION SELECT base FROM tickers WHERE exchange_id = ?1 AND base_asset_id IN (SELECT value FROM json_each(?2))")?;
    let names = s.query_map(params![bot.exchange_id, ids], |r| r.get::<_, Option<String>>(0))?.collect::<Result<Vec<_>, _>>()?;
    let splits = rows.iter().filter(|(asset, currency)| asset.is_some_and(|a| members.contains(&a)) || names.contains(&Some(currency.clone()))).count();
    if splits > 0 { r.push(format!("{splits} split(s) recorded for its assets (split-adjusted history is not supported by this engine yet)")); }
    Ok(())
}

/// The kinds of order a bot's history holds that this build does not read: special rows outside what
/// special_reasons admits, imported rows, and orders without an asset.
fn order_kind_reasons(c: &Connection, bot: &Bot, alpaca: bool, r: &mut Vec<String>) -> Result<(), EngineError> {
    let (imported, no_asset): (i64, i64) = c.query_row(
        "SELECT coalesce(sum(external_id LIKE 'imported_%'), 0), \
                coalesce(sum((transaction_type = 'REGULAR' OR status = 0) AND base_asset_id IS NULL), 0) FROM transactions WHERE bot_id = ?1",
        [bot.id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    // Sell rows are no refusal on any venue: off Alpaca the same normalized walk reads them (tests/eligibility.rs proves it
    // on Kraken and Binance), and bot_reasons_with runs it for such a history before accepting it.
    special_row_reasons(c, bot, alpaca, r)?;
    if imported > 0 { r.push(format!("{imported} imported row(s) in its history")); }
    if no_asset > 0 { r.push(format!("{no_asset} order(s) recorded without base_asset_id")); }
    Ok(())
}

/// The LIQUIDATION, REDEPLOY and REBALANCE rows this build does not read: on an Alpaca index bot those special_reasons
/// names; on a basket, and on an index bot off Alpaca, every LIQUIDATION and REDEPLOY row too (6c is Alpaca's).
fn special_row_reasons(c: &Connection, bot: &Bot, alpaca: bool, r: &mut Vec<String>) -> Result<(), EngineError> {
    if alpaca && bot.bot_type == "Bots::DcaIndex" { return special_reasons(c, bot, r); }
    // A rebalancing basket (6b), and an index bot on a crypto venue (plan CB T4): its REBALANCE rows are read as the Alpaca
    // index bot's are; the walk books them.
    if bot.bot_type == "Bots::DcaMultiAsset" || bot.bot_type == "Bots::DcaIndex" {
        let other: i64 = c.query_row("SELECT count(*) FROM transactions WHERE bot_id = ?1 AND transaction_type NOT IN ('REGULAR', 'REBALANCE')", [bot.id], |r| r.get(0))?;
        if other > 0 { r.push(format!("{other} LIQUIDATION/REDEPLOY row(s) in its history")); }
        return special_reasons(c, bot, r);
    }
    let other: i64 = c.query_row("SELECT count(*) FROM transactions WHERE bot_id = ?1 AND transaction_type <> 'REGULAR'", [bot.id], |r| r.get(0))?;
    if other > 0 { r.push(format!("{other} REBALANCE/LIQUIDATION/REDEPLOY row(s) in its history")); }
    Ok(())
}

/// The special rows of a bot's history the engine does not read (special_row_reasons, the rule its guard applies). The web
/// pages refuse a bot by it (web/bot/mod.rs `refusal`), so a page never refuses a history the engine reads. The guard's
/// other refusals stand at the start itself.
pub fn unread_special_rows(c: &Connection, bot: &Bot) -> Result<Vec<String>, EngineError> {
    let exchange: Option<String> = c.query_row("SELECT type FROM exchanges WHERE id = ?1", [bot.exchange_id], |r| r.get(0)).optional()?;
    let mut r = vec![];
    special_row_reasons(c, bot, exchange.as_deref() == Some("Exchanges::Alpaca"), &mut r)?;
    Ok(r)
}

/// What of an index bot's special history the walk does not read as Rails writes it. Waiting, pending and abandoned rows
/// are rails_work's; a rejected row that executed nothing is ignored, as Rails' `submitted` scope ignores it.
fn special_reasons(c: &Connection, bot: &Bot, r: &mut Vec<String>) -> Result<(), EngineError> {
    let (kinds, sides, statuses): (i64, i64, i64) = c.query_row(
        "SELECT coalesce(sum(transaction_type NOT IN ('LIQUIDATION', 'REBALANCE', 'REDEPLOY')), 0), \
                coalesce(sum((transaction_type = 'LIQUIDATION' AND side IS NOT 1) OR (transaction_type = 'REDEPLOY' AND side IS NOT 0) \
                             OR side IS NULL OR side NOT IN (0, 1)), 0), \
                coalesce(sum(external_status IS NULL OR external_status NOT IN (0, 1, 2, 3, 4)), 0) \
         FROM transactions WHERE bot_id = ?1 AND status = 0 AND transaction_type <> 'REGULAR'",
        [bot.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    if kinds > 0 { r.push(format!("{kinds} row(s) of an unsupported transaction type")); }
    if sides > 0 { r.push(format!("{sides} special row(s) with a side Rails never writes (LIQUIDATION buy, REDEPLOY sell or none)")); }
    if statuses > 0 { r.push(format!("{statuses} special row(s) with an unknown external status")); }
    let executed = crate::figures::fill::rejected_special_executions(c, bot.id).map_err(|e| EngineError::Data(format!("{e:?}")))?;
    if executed > 0 { r.push(format!("{executed} rejected LIQUIDATION/REDEPLOY/REBALANCE order(s) reporting an execution")); }
    Ok(())
}

/// Every member's pair on this venue, and the composition rows Rails' tick reads (bot_index_assets).
pub(crate) fn member_reasons(c: &Connection, bot: &Bot, alpaca: bool, members: &[i64], r: &mut Vec<String>) -> Result<(), EngineError> {
    let mut quote_checked = false;
    for &asset_id in members {
        let Some(t) = model::ticker_for_asset(c, bot, asset_id)? else {
            // An index bot's stored rows are not what it trades: every derivation (the tick, the resync, the rebalance) seats
            // members from this venue's tickers alone and marks every other row out of the index (index_allocatable.rb:69-102,
            // allocatable.rb:118-128), so a row without a ticker here, one a venue switch left, is a holding only. A basket's
            // members are its own settings and must trade here.
            if bot.bot_type != "Bots::DcaIndex" { r.push(format!("no ticker for the asset {asset_id} on this venue")); }
            continue;
        };
        // An unbounded integer column used as a rounding scale: refused here, before any leg is sized with it.
        if let Err(e) = t.scales() { r.push(e); }
        // Rails' crypto assets are category 'Cryptocurrency' (Exchange::Synchronizer); wrappers such as tokenized stocks carry
        // an instrument_type (Asset.mark_tokenized!) and may be split — outside the slice. Stocks and ETFs are category Stock,
        // typed by instrument_type (MarketData::CATEGORY_BY_TYPE, market_data.rb:377, :447), and trade on Alpaca only. A legacy
        // `alpaca_<uuid>` asset is never refreshed by the listings sync (market_data.rb:563-566), so nothing vouches for it.
        let (category, instrument, external_id): (Option<String>, Option<String>, Option<String>) = c.query_row(
            "SELECT category, instrument_type, external_id FROM assets WHERE id = ?1", [t.base_asset_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        let crypto = category.as_deref() == Some("Cryptocurrency") && instrument.is_none();
        let stock = alpaca && category.as_deref() == Some("Stock") && matches!(instrument.as_deref(), Some("stock" | "etf"))
            && !external_id.as_deref().unwrap_or_default().starts_with("alpaca_");
        if !crypto && !stock {
            r.push(format!("asset category {} / instrument {} (only plain cryptocurrencies, and stocks and ETFs on Alpaca)",
                           category.unwrap_or_default(), instrument.unwrap_or_default()));
        }
        // Both Alpaca catalogs import only USD-quoted crypto (MarketData.sync_alpaca_crypto_listings_from_deltabadger!,
        // Exchange::SyncAlpacaAssetsJob), and Exchanges::Alpaca#get_balances resolves the quote from USD only. One quote per bot.
        if alpaca && !quote_checked && t.quote_symbol != "USD" { r.push(format!("quote {} (Alpaca: only USD)", t.quote_symbol)); }
        quote_checked = true;
    }
    // Rails keeps a row per member (in_index) and per former member (exited: in_index false or NULL, outside the `in_index`
    // scope). An exited row is allowed. It is a holding only: the ledger walk counts its units, the split never
    // sees it (current_allocations reads in_index rows), and nothing buys it. Refusing it would let one mid-run delisting (the
    // tick marks the member exited) refuse the whole install and stop every bot. An in_index row for an asset outside
    // settings is an index asset.
    let mut s = c.prepare("SELECT asset_id FROM bot_index_assets WHERE bot_id = ?1 AND in_index = 1")?;
    let current = s.query_map([bot.id], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    let foreign = current.iter().filter(|a| !members.contains(a)).count();
    if bot.bot_type != "Bots::DcaIndex" && foreign > 0 { r.push(format!("index assets present ({foreign})")); }
    Ok(())
}

pub fn check_install(c: &Connection) -> Result<Report, EngineError> { install(c, None) }

/// check_install, also collecting each finding on its own when `items` is given, each with its identity: every reason of a
/// working bot as "bot {id}: {reason}", a read error with its error, every other problem line whole, and for every bot it
/// refuses, each member's own findings ("bot {id} asset {asset}: …", member_findings), so that one member already refused
/// cannot mask the same damage to another.
fn install(c: &Connection, mut items: Option<&mut std::collections::BTreeSet<String>>) -> Result<Report, EngineError> {
    let mut report = Report { eligible: vec![], problems: vec![], unreadable: vec![], notes: vec![] };
    let mut splits = SplitRows::new();
    let rules: i64 = c.query_row(&format!("SELECT count(*) FROM rules WHERE status IN ({})", model::working_list()), [], |r| r.get(0))?;
    if rules > 0 {
        report.problems.push(format!("{rules} active rule(s): rules run only in the full app"));
        if let Some(i) = items.as_deref_mut() { i.extend(report.problems.last().cloned()); }
    }
    // Every status, archived and deleted included: archiving or deleting a bot leaves its orders at the venue,
    // and Rails keeps accounting for them, so their pending work refuses the install like any other bot's.
    let mut s = c.prepare("SELECT id FROM bots ORDER BY id")?;
    let ids = s.query_map([], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
    for id in ids {
        let (bot, reasons) = match model::load_bot(c, id).and_then(|b| bot_reasons_with(c, &b, &mut splits).map(|r| (b, r))) {
            Ok(x) => x,
            Err(e) => {
                if let Some(i) = items.as_deref_mut() { i.insert(format!("bot {id}: unreadable ({e:?})")); member_findings(c, id, i)?; }
                report.unreadable.push((id, format!("{e:?}")));
                continue;
            }
        };
        let working = BOT_WORKING.contains(&bot.status);
        if reasons.is_empty() {
            if working { report.eligible.push(id); }
            continue;
        }
        if working {
            report.problems.push(format!("bot {id} ({}): {}", bot.status.label(), reasons.join(", ")));
            if let Some(i) = items.as_deref_mut() { i.extend(reasons.iter().map(|r| format!("bot {id}: {r}"))); member_findings(c, id, i)?; }
            continue;
        }
        // UNBALANCED alone refuses ticking only: a stopped bot does not tick, and its swap still places, polls and settles
        // (Rails' rebalance_targets: `rebalance_pending? || allocations_balanced?`). Its REBALANCE orders are this engine's.
        let rebalances = rebalances_here(&bot);
        let swap_only = rebalances && reasons.iter().all(|r| r.starts_with(UNBALANCED));
        let outstanding: i64 = c.query_row("SELECT count(*) FROM transactions WHERE bot_id = ?1 AND status = 0 AND external_status IN (0, 1) \
                                            AND NOT (?2 AND transaction_type = 'REBALANCE')", params![id, swap_only], |r| r.get(0))?;
        let mut work = rails_work(c, &bot)?;
        // A stopped bot still rebalances (run::run_rebalances), so whatever else it carries must be within the slice.
        if super::rebalance::candidate(&bot) && rebalances && !swap_only { work.push("rebalancing".into()); }
        if outstanding > 0 { work.push(format!("{outstanding} outstanding order(s)")); }
        if !work.is_empty() {
            report.problems.push(format!("bot {id} ({}) has work only the full app handles ({}); outside the slice: {}", bot.status.label(), work.join(", "), reasons.join(", ")));
            if let Some(i) = items.as_deref_mut() { i.extend(report.problems.last().cloned()); member_findings(c, id, i)?; }
        }
    }
    Ok(report)
}

/// `check` and the takeover: check_install, plus every bot this engine would run whose reference data is past its bound
/// (staleness.rs). A source with no stamp is logged and noted, never refused. The running engine does not call this: there
/// the tick refuses a stale bot (tick.rs), and the engine goes on.
pub fn check_install_at(c: &Connection, now: DateTime<Utc>) -> Result<Report, EngineError> {
    use super::staleness::{verdict, Verdict};
    let mut report = check_install(c)?;
    let mut fresh = vec![];
    for id in std::mem::take(&mut report.eligible) {
        let bot = model::load_bot(c, id)?;
        match verdict(c, &bot, now)? {
            Verdict::Stale(s) => { report.problems.push(format!("bot {id} ({}): {}", bot.status.label(), s.message)); continue; }
            Verdict::Unknown(s) => {
                let line = format!("bot {id} ({}): {}", bot.status.label(), s.message);
                super::log(&format!("[check] {line}"));
                report.notes.push(line);
            }
            Verdict::Fresh => {}
        }
        fresh.push(id);
    }
    report.eligible = fresh;
    Ok(report)
}

/// Why `check` and `serve` refuse an install, or why a write to `bots` must not commit (`guard`).
#[derive(Debug)]
pub enum Refusal {
    /// `check`'s problem lines, each naming the bot and the reason: "bot 7 (scheduled): price_limited".
    Ineligible(Vec<String>),
    /// Bot rows this build cannot read. Takeover refuses them, so no write may leave one behind.
    Unreadable(Vec<(i64, String)>),
    /// Bots with an unresolved order whose composition, asset, exchange or quote a write would change (`placement::stranded`).
    Reconciling(Vec<String>),
    /// What this build cannot trade, in `alpaca::preflight`'s words: a venue with no Rust code, no key.
    Untradable(Vec<String>),
    /// The check itself could not run.
    Failed(EngineError),
}

impl Refusal {
    /// `check`'s words, exactly: what `check` and `serve` print after "deltabadger: ".
    pub fn message(&self) -> String {
        match self {
            Self::Ineligible(problems) => format!("this install uses things only the full app runs:\n{}", problems.join("\n")),
            Self::Unreadable(rows) => format!("unreadable bot rows: {rows:?}"),
            Self::Reconciling(lines) => format!("an order is still being reconciled:\n{}", lines.join("\n")),
            Self::Untradable(problems) => format!("refusing to take this install over:\n{}", problems.join("\n")),
            Self::Failed(e) => format!("{e:?}"),
        }
    }

    /// The same lines on one line: the reason a refused write carries (the web's 422, `engine.write_refused`).
    pub fn reason(&self) -> String {
        match self {
            Self::Ineligible(lines) | Self::Reconciling(lines) | Self::Untradable(lines) => lines.join("; "),
            Self::Unreadable(rows) => rows.iter().map(|(id, e)| format!("bot {id}: unreadable ({e})")).collect::<Vec<_>>().join("; "),
            Self::Failed(_) => "the check could not be completed".into(), // the error itself is logged by `guard`
        }
    }
}

impl Report {
    /// The eligible bots, or why the install is refused: problems first, then unreadable rows, as `check` reports them.
    pub fn refusal(self) -> Result<Vec<i64>, Refusal> {
        if !self.problems.is_empty() { return Err(Refusal::Ineligible(self.problems)); }
        if !self.unreadable.is_empty() { return Err(Refusal::Unreadable(self.unreadable)); }
        Ok(self.eligible)
    }
}

/// THE guard. Every writer of `bots` outside the engine (the web UI now; MCP and REST
/// later) calls it inside its own `BEGIN IMMEDIATE` transaction (`model::immediate`), after its statements and before
/// `commit`, with the instance's `Cipher` (the web: `app.cipher`). `Ok`: commit. `Err`: roll back, and answer in the
/// caller's own shape (the web: 422 with `engine.write_refused` carrying `reason()`). It refuses, in this order: what
/// the engine does not run (`check`'s words); a write that would strand an unresolved order; what this build cannot
/// trade (`preflight`'s words: Kraken or Alpaca, a key present and readable). A write that skips it and makes the
/// install ineligible is caught by the engine's pass in one process (`run::step`'s debug assertion). When the check
/// itself fails, the real error is logged with `bot_id`, or for a background job when absent; `reason()` stays generic.
pub fn guard(tx: &Connection, cipher: &Cipher, bot_id: Option<i64>) -> Result<(), Refusal> {
    let failed = |e: EngineError| {
        let writer = bot_id.map_or_else(|| "a background job's write".to_string(), |id| format!("bot {id}"));
        crate::engine::log(&format!("guard failed for {writer}: {e:?}"));
        Refusal::Failed(e)
    };
    super::provider::bind_cipher(tx,cipher).map_err(failed)?;
    check_install(tx).map_err(failed)?.refusal()?;
    reconciling(tx).map_err(failed)??;
    crate::venue::alpaca::preflight(tx, cipher).map(|_| ()).map_err(Refusal::Untradable)
}

/// Each member's own findings for a bot check_install refuses, named by member: its member_reasons (or their read error), or,
/// for a bot row that cannot be loaded, the member's reference rows themselves, so any change to them is new. Members are
/// what Touched::relevant reads: the allocations' keys and bot_index_assets.
fn member_findings(c: &Connection, id: i64, items: &mut std::collections::BTreeSet<String>) -> Result<(), EngineError> {
    let mut s = c.prepare("SELECT CAST(a.key AS INTEGER) FROM bots b, json_each(b.settings, '$.allocations') a WHERE b.id = ?1 \
                           UNION SELECT asset_id FROM bot_index_assets WHERE bot_id = ?1 ORDER BY 1")?;
    let members = s.query_map([id], |r| r.get::<_, Option<i64>>(0))?.collect::<Result<Vec<_>, _>>()?;
    let loaded = model::load_bot(c, id);
    for m in members.into_iter().flatten() {
        match &loaded {
            Ok(bot) => {
                let alpaca = model::exchange_type(c, bot).ok().as_deref() == Some("Exchanges::Alpaca");
                let mut r = vec![];
                match member_reasons(c, bot, alpaca, &[m], &mut r) {
                    Ok(()) => items.extend(r.into_iter().map(|reason| format!("bot {id} asset {m}: {reason}"))),
                    Err(e) => { items.insert(format!("bot {id} asset {m}: unreadable ({e:?})")); }
                }
            }
            Err(_) => {
                let mut s = c.prepare("SELECT coalesce(a.category, '') || '|' || coalesce(a.instrument_type, '') || '|' || coalesce(a.external_id, '') || '|' || \
                    coalesce((SELECT group_concat(t.id || ':' || t.ticker || ':' || t.base || ':' || t.quote || ':' || coalesce(t.base_decimals, '') || ':' || \
                    coalesce(t.quote_decimals, '') || ':' || coalesce(t.price_decimals, '') || ':' || coalesce(t.available, '') || ':' || t.quote_asset_id, ';') \
                    FROM tickers t WHERE t.base_asset_id = a.id AND t.exchange_id = (SELECT exchange_id FROM bots WHERE id = ?2)), '') \
                    FROM assets a WHERE a.id = ?1")?;
                let rows: Option<String> = s.query_row(params![m, id], |r| r.get(0)).optional()?;
                items.insert(format!("bot {id} asset {m}: rows {}", rows.unwrap_or_else(|| "missing".into())));
            }
        }
    }
    Ok(())
}

/// check_install's findings one at a time: each reason of each working bot, every other problem line whole.
pub fn install_findings(c: &Connection) -> Result<std::collections::BTreeSet<String>, EngineError> {
    let mut items = std::collections::BTreeSet::new();
    install(c, Some(&mut items))?;
    Ok(items)
}

/// `guard` for a reference-data write (jobs::import::in_transaction). What check_install refuses is judged by what the
/// write adds: a finding the install already had (`before`: install_findings on the state the write started from) does not
/// block it, so one bot already outside the slice cannot freeze every other bot's reference data, while a write that gives
/// any bot a new reason is refused. An unresolved order and preflight's own refusals (venue, key) refuse as in `guard`.
pub fn guard_reference_write(tx: &Connection, cipher: &Cipher, before: impl FnOnce() -> Result<std::collections::BTreeSet<String>, EngineError>) -> Result<(), Refusal> {
    let failed = |e: EngineError| {
        crate::engine::log(&format!("guard failed for a background job's write: {e:?}"));
        Refusal::Failed(e)
    };
    super::provider::bind_cipher(tx, cipher).map_err(failed)?;
    let mut after = std::collections::BTreeSet::new();
    let report = install(tx, Some(&mut after)).map_err(failed)?;
    let whole: Vec<String> = report.problems.iter().cloned()
        .chain(report.unreadable.iter().map(|(id, e)| format!("bot {id}: unreadable ({e})"))).collect();
    if !whole.is_empty() && after.difference(&before().map_err(failed)?).next().is_some() { report.refusal()?; }
    reconciling(tx).map_err(failed)??;
    // preflight repeats check_install's lines, judged above; what is left is its own.
    match crate::venue::alpaca::preflight(tx, cipher) {
        Err(lines) => {
            let own: Vec<String> = lines.into_iter().filter(|l| !whole.contains(l)).collect();
            if own.is_empty() { Ok(()) } else { Err(Refusal::Untradable(own)) }
        }
        Ok(_) => Ok(()),
    }
}

fn reconciling(tx: &Connection) -> Result<Result<(), Refusal>, EngineError> {
    let stranded = placement::stranded(tx)?;
    if stranded.is_empty() { return Ok(Ok(())); }
    Ok(Err(Refusal::Reconciling(stranded.iter().map(|id| {
        format!("bot {id}: an order is still being reconciled; its composition, asset, exchange and quote cannot change until it settles")
    }).collect())))
}

/// Bots::DcaIndex within what this build ports: a data-api install (MarketData.get_top_coins reads the local row only there;
/// on a CoinGecko one it calls CoinGecko live), USD on Alpaca, a Top coins or category index, a readable count and
/// flattening. A missing `indices` row is the tick's own Failure ("Index not found"), never an install refusal. Returns the
/// assets of every bot_index_assets row, in or out of the index, for member_reasons.
fn index_reasons(c: &Connection, bot: &Bot, alpaca: bool, r: &mut Vec<String>) -> Result<Vec<i64>, EngineError> {
    if super::provider::config(c)?.is_none() { r.push("index requires a configured deltabadger market-data provider".into()); }
    // member_reasons checks the quote only through a member's ticker, so a bot with no members yet is checked here.
    if alpaca && model::quote_symbol(c, bot)? != "USD" { r.push(format!("quote {} (Alpaca: only USD)", model::quote_symbol(c, bot)?)); }
    if !matches!(bot.index_type(), Some("top" | "category")) { r.push(format!("index_type {:?} (only Top coins or a category index)", bot.index_type())); }
    if !bot.num_coins().is_some_and(|n| (1..=1000).contains(&n)) { r.push("num_coins is not a positive integer".into()); }
    if !bot.allocation_flattening().is_some_and(|f| (0.0..=1.0).contains(&f)) { r.push("allocation_flattening is not a number in 0..1".into()); }
    let mut s = c.prepare("SELECT asset_id FROM bot_index_assets WHERE bot_id = ?1 ORDER BY id")?;
    let ids = s.query_map([bot.id], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}
