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
const PENDING_KEYS: [&str; 3] = ["rebalance_pending", "liquidation_pending", "redeploy_pending"];

fn set(v: &Value) -> bool { !matches!(v, Value::Null | Value::Bool(false)) && v != "false" && v != 0 && v != "" }

/// Work only Rails carries out: Bot::LiquidationState#liquidation_in_flight?, Bot::Composition::Redeployable#redeploy_in_flight?,
/// Bot::EvaluateRebalancersJob's candidates. Each rule covers the statuses Rails handles it for. The pending keys stay on
/// every status: Rails' automatic legs skip deleted/archived bots, but its halt resolutions (Bots::*ResolutionsController,
/// Bot::Resolve*Job) take any bot, and an archived bot's page offers them.
fn rails_work(c: &Connection, bot: &Bot) -> Result<Vec<String>, EngineError> {
    let mut r = vec![];
    // Deliberately broader than Rails' automatic jobs: a false refusal costs one check message, a takeover mid-liquidation costs money.
    for key in PENDING_KEYS { if bot.transient.get(key).is_some_and(|v| !v.is_null()) { r.push(key.to_string()); } }
    // Bot::EvaluateRebalancersJob#candidates: REBALANCEABLE_TYPES, `.where.not(status: %i[deleted archived])`. A stopped bot still rebalances.
    let rebalanceable = matches!(bot.bot_type.as_str(), "Bots::DcaIndex" | "Bots::DcaMultiAsset") && !matches!(bot.status, BotStatus::Deleted | BotStatus::Archived);
    if rebalanceable && bot.settings.get("rebalance_enabled").is_some_and(set) { r.push("rebalance_enabled".into()); }
    let non_regular: i64 = c.query_row(
        "SELECT count(*) FROM transactions WHERE bot_id = ?1 AND transaction_type <> 'REGULAR' AND status = 0 AND external_status IN (0, 1)",
        [bot.id], |r| r.get(0))?;
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
    Ok(r)
}

pub fn bot_reasons(c: &Connection, bot: &Bot) -> Result<Vec<String>, EngineError> {
    let mut r = rails_work(c, bot)?;
    if bot.bot_type != "Bots::DcaMultiAsset" { r.push(format!("type {} (only DCA baskets)", bot.bot_type)); }
    let exchange: Option<String> = c.query_row("SELECT type FROM exchanges WHERE id = ?1", [bot.exchange_id], |r| r.get(0)).optional()?;
    if !matches!(exchange.as_deref(), Some("Exchanges::Kraken" | "Exchanges::Alpaca")) {
        r.push(format!("exchange {} (only Kraken and Alpaca)", exchange.as_deref().unwrap_or_default()));
    }
    let kraken = exchange.as_deref() == Some("Exchanges::Kraken");
    let alpaca = exchange.as_deref() == Some("Exchanges::Alpaca");
    let members = composition_reasons(bot, kraken, &mut r);
    match bot.settings.get("direction") { None | Some(Value::Null) => {}, Some(v) if v == "buying" => {}, Some(d) => r.push(format!("direction {d}")) }
    if let Some(obj) = bot.settings.as_object() {
        for (k, v) in obj {
            let flag = k.ends_with("_limited") || k.ends_with("_ordered") || k.ends_with("_intervaled") || k == "start_time_enabled";
            if flag && set(v) && !SUPPORTED_FLAGS.contains(&k.as_str()) { r.push(k.clone()); }
        }
    }
    if bot.settings.get("smart_intervaled").is_some_and(set) && !bot.smart_quote_amount().is_some_and(|a| a > 0.0) {
        r.push("smart interval amount missing, not a JSON number, or not positive".into());
    }
    if bot.limit_ordered() && bot.limit_distance().is_none() { r.push("limit_order_pcnt_distance is not a number".into()); }
    if BOT_WORKING.contains(&bot.status) && bot.started_at_us.is_none() { r.push("started_at missing (never ticks)".into()); }
    if bot.interval().is_none() { r.push("interval".into()); }
    if !bot.quote_amount().is_some_and(|q| q > 0.0) { r.push("quote_amount".into()); }
    if bot.restatement_generation > 0 { r.push("restated prices".into()); }
    let wash: Option<Option<bool>> = c.query_row("SELECT wash_sale_enabled FROM users WHERE id = ?1", [bot.user_id], |r| r.get(0)).optional()?;
    match wash { None => r.push("user not found".into()), Some(Some(true)) => r.push("wash_sale enabled for the user".into()), _ => {} }
    if bot.quote_amount_limited() {
        if kraken { r.push("quote_amount_limited (Kraken: amount limits are not supported by this engine yet)".into()); }
        if let Err(e) = bot.quote_amount_limit() { r.push(e); }
        if let Err(e) = bot.quote_amount_limit_enabled_at_us() { r.push(e); }
    }
    if bot.merged_history() { r.push("merged history (merged_history_until_id)".into()); }
    history_reasons(c, bot, &members, &mut r)?;
    member_reasons(c, bot, alpaca, &members, &mut r)?;
    Ok(r)
}

/// settings.allocations: what Rails would start, within what this build ports (manual weights, 1..MAX_ASSETS members;
/// Kraken keeps one member). Returns the member asset ids in settings order.
fn composition_reasons(bot: &Bot, kraken: bool, r: &mut Vec<String>) -> Vec<i64> {
    if bot.weighting() != "manual" { r.push(format!("weighting {} (market-cap weights are not supported by this engine yet)", bot.weighting())); }
    let Some(a) = bot.allocations() else {
        r.push("allocations: an asset id or a weight this build does not read".into());
        return vec![];
    };
    if a.is_empty() { r.push("allocations: none".into()); }
    if a.len() > MAX_ASSETS { r.push(format!("allocations: {} assets (at most {MAX_ASSETS})", a.len())); }
    if kraken && a.len() > 1 { r.push(format!("allocations: {} assets (Kraken: only one-asset bots are supported by this engine yet)", a.len())); }
    // Bots::DcaMultiAsset::Allocatable#allocations_balanced?. The tolerance dwarfs the difference between this plain sum and
    // Ruby's compensated one.
    let total: f64 = a.iter().map(|(_, w)| w).sum();
    if a.iter().any(|(_, w)| *w < 0.0) || (total - 1.0).abs() > ALLOCATION_TOLERANCE {
        r.push(format!("allocations: weights sum to {total}, not 1"));
    }
    // Keys that name one id ("7", "07") would make the basket carry one asset as two members.
    let ids: Vec<i64> = a.into_iter().map(|(id, _)| id).collect();
    for (i, id) in ids.iter().enumerate() {
        if ids[..i].contains(id) && !ids[i + 1..].contains(id) { r.push(format!("allocations: asset {id} listed more than once")); }
    }
    ids
}

/// The ledger walk this build ports (basket::holdings) holds REGULAR buys recorded with their asset, and nothing a
/// recorded split would restate.
fn history_reasons(c: &Connection, bot: &Bot, members: &[i64], r: &mut Vec<String>) -> Result<(), EngineError> {
    let (sells, other, imported, no_asset): (i64, i64, i64, i64) = c.query_row(
        "SELECT coalesce(sum(side = 1), 0), coalesce(sum(transaction_type <> 'REGULAR'), 0), coalesce(sum(external_id LIKE 'imported_%'), 0), \
                coalesce(sum(transaction_type = 'REGULAR' AND base_asset_id IS NULL), 0) FROM transactions WHERE bot_id = ?1",
        [bot.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
    if sells > 0 { r.push(format!("{sells} sell order(s) in its history")); }
    if other > 0 { r.push(format!("{other} REBALANCE/LIQUIDATION/REDEPLOY row(s) in its history")); }
    if imported > 0 { r.push(format!("{imported} imported row(s) in its history")); }
    if no_asset > 0 { r.push(format!("{no_asset} order(s) recorded without base_asset_id")); }
    // Bot::Restatable#grouped_split_rows applies a split recorded in account_transactions (corporate_action 'split') inside
    // the walk; the ported walk does not. Crypto never splits, so this refuses only what this build does not port. Broader than Rails'
    // match (any member's asset id, symbol or venue spelling), so it can only refuse more.
    let ids = serde_json::to_string(members).expect("ids serialise");
    let splits: i64 = c.query_row(
        "SELECT count(*) FROM account_transactions a WHERE a.user_id = ?1 AND a.exchange_id = ?2 AND a.entry_type = 15 \
         AND (CASE WHEN json_valid(a.raw_data) THEN json_extract(a.raw_data, '$.corporate_action') END) = 'split' \
         AND (a.base_asset_id IN (SELECT value FROM json_each(?3)) \
              OR a.base_currency IN (SELECT symbol FROM assets WHERE id IN (SELECT value FROM json_each(?3))) \
              OR a.base_currency IN (SELECT base FROM tickers WHERE exchange_id = ?2 AND base_asset_id IN (SELECT value FROM json_each(?3))))",
        params![bot.user_id, bot.exchange_id, ids], |r| r.get(0))?;
    if splits > 0 { r.push(format!("{splits} split(s) recorded for its assets (split-adjusted history is not supported by this engine yet)")); }
    Ok(())
}

/// Every member's pair on this venue, and the composition rows Rails' tick reads (bot_index_assets).
fn member_reasons(c: &Connection, bot: &Bot, alpaca: bool, members: &[i64], r: &mut Vec<String>) -> Result<(), EngineError> {
    let mut quote_checked = false;
    for &asset_id in members {
        let Some(t) = model::ticker_for_asset(c, bot, asset_id)? else {
            r.push(format!("no ticker for the asset {asset_id} on this venue"));
            continue;
        };
        // Rails' crypto assets are category 'Cryptocurrency' (Exchange::Synchronizer); wrappers such as tokenized stocks carry
        // an instrument_type (Asset.mark_tokenized!) and may be split — outside the slice.
        let (category, instrument): (Option<String>, Option<String>) = c.query_row(
            "SELECT category, instrument_type FROM assets WHERE id = ?1", [t.base_asset_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        if category.as_deref() != Some("Cryptocurrency") || instrument.is_some() {
            r.push(format!("asset category {} / instrument {} (only plain cryptocurrencies)", category.unwrap_or_default(), instrument.unwrap_or_default()));
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
    if foreign > 0 { r.push(format!("index assets present ({foreign})")); }
    Ok(())
}

pub fn check_install(c: &Connection) -> Result<Report, EngineError> {
    let mut report = Report { eligible: vec![], problems: vec![], unreadable: vec![], notes: vec![] };
    let rules: i64 = c.query_row(&format!("SELECT count(*) FROM rules WHERE status IN ({})", model::working_list()), [], |r| r.get(0))?;
    if rules > 0 { report.problems.push(format!("{rules} active rule(s): rules run only in the full app")); }
    // Every status, archived and deleted included: archiving or deleting a bot leaves its orders at the venue,
    // and Rails keeps accounting for them, so their pending work refuses the install like any other bot's.
    let mut s = c.prepare("SELECT id FROM bots ORDER BY id")?;
    let ids = s.query_map([], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
    for id in ids {
        let (bot, reasons) = match model::load_bot(c, id).and_then(|b| bot_reasons(c, &b).map(|r| (b, r))) {
            Ok(x) => x,
            Err(e) => { report.unreadable.push((id, format!("{e:?}"))); continue; }
        };
        let working = BOT_WORKING.contains(&bot.status);
        if reasons.is_empty() {
            if working { report.eligible.push(id); }
            continue;
        }
        if working {
            report.problems.push(format!("bot {id} ({}): {}", bot.status.label(), reasons.join(", ")));
            continue;
        }
        let outstanding: i64 = c.query_row("SELECT count(*) FROM transactions WHERE bot_id = ?1 AND status = 0 AND external_status IN (0, 1)", [id], |r| r.get(0))?;
        let mut work = rails_work(c, &bot)?;
        if outstanding > 0 { work.push(format!("{outstanding} outstanding order(s)")); }
        if !work.is_empty() {
            report.problems.push(format!("bot {id} ({}) has work only the full app handles ({}); outside the slice: {}", bot.status.label(), work.join(", "), reasons.join(", ")));
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
    /// Bots whose unresolved order would no longer match them: recovery, `handback` and `resolve-placement` read the
    /// intent against the bot's current asset, exchange and quote (`placement::stranded`).
    Reconciling(Vec<String>),
    /// What this build cannot trade, in `alpaca::preflight`'s words: a venue other than Alpaca, a live key, no key.
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
/// trade (`preflight`'s words: Alpaca paper only, a key present and readable). A write that skips it and makes the
/// install ineligible is caught by the engine's pass in one process (`run::step`'s debug assertion). When the check
/// itself fails, the real error is logged with `bot_id` and `reason()` stays generic.
pub fn guard(tx: &Connection, cipher: &Cipher, bot_id: i64) -> Result<(), Refusal> {
    let failed = |e: EngineError| { crate::engine::log(&format!("guard failed for bot {bot_id}: {e:?}")); Refusal::Failed(e) };
    check_install(tx).map_err(failed)?.refusal()?;
    let stranded = placement::stranded(tx).map_err(failed)?;
    if !stranded.is_empty() {
        return Err(Refusal::Reconciling(stranded.iter().map(|id| {
            format!("bot {id}: an order is still being reconciled; its asset, exchange and quote cannot change until it settles")
        }).collect()));
    }
    crate::venue::alpaca::preflight(tx, cipher).map(|_| ()).map_err(Refusal::Untradable)
}
