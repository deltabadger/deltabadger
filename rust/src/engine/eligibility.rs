//! What the engine may run (spec §3 Eligibility). Anything outside the slice is refused and named.
//! Refusal reads flags by Ruby TRUTHINESS (anything set), not by the `== true` the supported readers
//! use: a flag Rails might treat as on must never be quietly ignored.
use super::model::{self, Bot};
use super::EngineError;
use crate::enums::{BotStatus, BOT_WORKING};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

/// `unreadable`: bots whose rows this build cannot read. Takeover refuses them; the running engine skips them.
pub struct Report { pub eligible: Vec<i64>, pub problems: Vec<String>, pub unreadable: Vec<(i64, String)> }

const SUPPORTED_FLAGS: [&str; 2] = ["limit_ordered", "smart_intervaled"];
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
    if bot.bot_type != "Bots::DcaMultiAsset" { r.push(format!("type {} (only one-asset DCA baskets)", bot.bot_type)); }
    let exchange: Option<String> = c.query_row("SELECT type FROM exchanges WHERE id = ?1", [bot.exchange_id], |r| r.get(0)).optional()?;
    if !matches!(exchange.as_deref(), Some("Exchanges::Kraken" | "Exchanges::Alpaca")) {
        r.push(format!("exchange {} (only Kraken and Alpaca)", exchange.as_deref().unwrap_or_default()));
    }
    if bot.asset_ids().len() != 1 { r.push(format!("allocations: {} assets (only one)", bot.asset_ids().len())); }
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
    // Bots::DcaMultiAsset#set_tickers adds bot_index_assets to the asset list: not modelled in the slice.
    // Rails keeps a row for the bot's own allocated asset too; set_tickers uniq's it away, so only other assets count.
    let own = bot.asset_ids();
    let mut s = c.prepare("SELECT asset_id FROM bot_index_assets WHERE bot_id = ?1")?;
    let index_assets = s.query_map([bot.id], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?.into_iter().filter(|a| !own.contains(a)).count();
    if index_assets > 0 { r.push(format!("index assets present ({index_assets})")); }
    match model::ticker_for(c, bot)? {
        None => r.push("no ticker for the asset on this venue".into()),
        Some(t) => {
            // Rails' crypto assets are category 'Cryptocurrency' (Exchange::Synchronizer); wrappers such as tokenized
            // stocks carry an instrument_type (Asset.mark_tokenized!) and may be split — outside the slice.
            let (category, instrument): (Option<String>, Option<String>) = c.query_row(
                "SELECT category, instrument_type FROM assets WHERE id = ?1", [t.base_asset_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            if category.as_deref() != Some("Cryptocurrency") || instrument.is_some() {
                r.push(format!("asset category {} / instrument {} (only plain cryptocurrencies)", category.unwrap_or_default(), instrument.unwrap_or_default()));
            }
            // Both Alpaca catalogs import only USD-quoted crypto (MarketData.sync_alpaca_crypto_listings_from_deltabadger!,
            // Exchange::SyncAlpacaAssetsJob), and Exchanges::Alpaca#get_balances resolves the quote from USD only.
            if exchange.as_deref() == Some("Exchanges::Alpaca") && t.quote_symbol != "USD" {
                r.push(format!("quote {} (Alpaca: only USD)", t.quote_symbol));
            }
        }
    }
    Ok(r)
}

pub fn check_install(c: &Connection) -> Result<Report, EngineError> {
    let mut report = Report { eligible: vec![], problems: vec![], unreadable: vec![] };
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
