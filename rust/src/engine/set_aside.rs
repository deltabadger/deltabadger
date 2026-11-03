//! PLAN-B2 R3, R4, r1.4, r1.12 (B2-5): a working bot this build refuses is **set aside** at every start: stopped the way
//! Rails stops a bot for a blocking failure (tick::stop_owing_mail), with the reasons in the owner's words, and the app
//! starts. A working withdrawal rule on an exchange without withdrawals is stopped the same way. Then the same check that
//! refused it runs again on the same transaction: what is still refused (work in flight, a row or key this build can't
//! read) is a hard stop, and the caller rolls the transaction back, so nothing is written.
//! D-UPGRADE-SET-ASIDE: Rails would have kept trading such a bot; 3.0 stops it and says why.
use super::eligibility::{self, Allocations, Feature, History, Kind, Reason, Report, Work};
use super::EngineError;
use crate::crypto::Cipher;
use crate::enums::{RuleStatus, BOT_WORKING};
use crate::venue::alpaca::{self, Untradable};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;

/// transient_data key: {"reasons": [sentence, …], "at": time}. Written with the stop; removed by the bot's next start.
pub const KEY: &str = "rust_set_aside";
/// The bot's stop_message_key (C2), a Rust-only text (web::i18n) taking %{reason}.
pub const STOP_KEY: &str = "bot.status.set_aside";

/// One bot set aside: its id, its name as the owner sees it, and the reasons in the owner's words.
#[derive(Debug, Clone, PartialEq)]
pub struct Aside { pub id: i64, pub name: String, pub reasons: Vec<String> }

/// What a start sets aside, and the report of the state it leaves (the one the engine then runs).
pub struct Plan { pub bots: Vec<Aside>, pub rules: Vec<i64>, pub report: Report }

/// A hard stop: C6's lines for the owner, one per bot, and the check's own lines for support (the log, never the page).
#[derive(Debug)]
pub struct Hard { pub lines: Vec<String>, pub details: Vec<String> }

/// Set aside every candidate on `tx`, then check again. `report` is the check the start runs (handover::startup_report,
/// or check_install_at for `check`); `cipher` None skips the key checks (`check` without SECRET_KEY_BASE). Ok(Err(lines)):
/// a hard stop; the caller must roll `tx` back.
pub fn apply(tx: &Connection, cipher: Option<&Cipher>, report: &dyn Fn(&Connection) -> Result<Report, EngineError>, now: DateTime<Utc>)
    -> Result<Result<Plan, Hard>, EngineError> {
    let first = report(tx)?;
    // Candidates: each working bot the check refuses, with its reasons, and each working bot whose exchange has no Rust code
    // or that has no key. A key that does not read is no candidate (it means a wrong secret): it stays refused below.
    let mut candidates: Vec<(i64, Vec<String>)> = vec![];
    for (id, reasons) in &first.refused {
        let mut said = vec![];
        for r in reasons { push_unique(&mut said, sentence(tx, *id, r)?); }
        candidates.push((*id, said));
    }
    if let Some(cipher) = cipher {
        let working: Vec<i64> = first.eligible.iter().copied().chain(first.refused.iter().map(|(id, _)| *id)).collect();
        for (id, why) in alpaca::untradable(tx, cipher, &working) {
            let said = match why { Untradable::Missing(name) => no_key(name), Untradable::NotConnected(_) => exchange(tx, id)?, Untradable::Unreadable(_) => continue };
            match candidates.iter_mut().find(|(c, _)| *c == id) { Some((_, s)) => push_unique(s, said), None => candidates.push((id, vec![said])) }
        }
    }
    candidates.sort_by_key(|(id, _)| *id);
    let mut bots = vec![];
    for (id, reasons) in candidates {
        let status: i64 = tx.query_row("SELECT status FROM bots WHERE id = ?1", [id], |r| r.get(0))?;
        if !BOT_WORKING.iter().any(|s| *s as i64 == status) { continue; }
        let reason = reasons.join(" ");
        let aside = json!({ "reasons": reasons, "at": crate::codec::format_time(now) });
        // Its own mail (notice::SET_ASIDE), not Rails' stopped-by-error one: nothing was rejected by the exchange.
        super::tick::stop_setting_aside(tx, id, STOP_KEY, (super::notice::SET_ASIDE, super::notice::stopped_marker(&reason, now)), (KEY, aside), &reason, now)?;
        bots.push(Aside { id, name: name(tx, id)?, reasons });
    }
    for id in &first.rules {
        let n = tx.execute("UPDATE rules SET status = ?1, updated_at = ?2 WHERE id = ?3 AND status IN (?4, ?5, ?6, ?7)",
            params![RuleStatus::Stopped as i64, crate::codec::format_time(now), id, RuleStatus::Scheduled as i64, RuleStatus::Executing as i64,
                    RuleStatus::Retrying as i64, RuleStatus::Waiting as i64])?;
        // RuleLog status failed (rule_log.rb: pending success failed transient), as Rails' log_failed.
        if n > 0 { super::rules::log(tx, *id, None, 2, &rule_stopped(tx, *id)?, &json!({}), now)?; }
    }
    let rules = first.rules.clone();
    // The same check again, on the state just written: what it still refuses cannot be set aside.
    let after = report(tx)?;
    let mut left: Vec<(i64, Vec<String>)> = vec![];
    let add = |left: &mut Vec<(i64, Vec<String>)>, id: i64, said: String| match left.iter_mut().find(|(b, _)| *b == id) {
        Some((_, s)) => push_unique(s, said), None => left.push((id, vec![said])),
    };
    for (id, reasons) in after.refused.iter().chain(&after.held) {
        let work: Vec<&Reason> = reasons.iter().filter(|r| matches!(r.kind, Kind::Work(_))).collect();
        for r in if work.is_empty() { reasons.iter().collect() } else { work } { add(&mut left, *id, sentence(tx, *id, r)?); }
    }
    // A bot set aside with an order whose outcome is unknown would still have it recovered and polled once stopped: work
    // in flight, as an open order is (Codex review).
    for aside in &bots {
        let intent: bool = tx.query_row("SELECT json_type(transient_data, '$.rust_placement') IS NOT NULL AND json_type(transient_data, '$.rust_placement') <> 'null' FROM bots WHERE id = ?1",
                                        [aside.id], |r| r.get(0))?;
        if intent { add(&mut left, aside.id, "It has an order whose result isn't known yet.".into()); }
    }
    // Support's lines read the install as it is (the stops roll back): the first check's, then preflight's on what is left.
    let mut details = first.problems.clone();
    details.extend(first.unreadable.iter().map(|(id, e)| format!("bot {id}: unreadable ({e})")));
    for (id, _) in &after.unreadable { add(&mut left, *id, "This bot's data can't be read.".into()); }
    if let Some(cipher) = cipher {
        let ids = alpaca::with_owing(tx, after.eligible.clone())?;
        for (id, why) in alpaca::untradable(tx, cipher, &ids) {
            details.push(format!("bot {id}: {}", why.text()));
            let said = match why {
                Untradable::Missing(name) => format!("It has open orders, and there's no {name} API key to look after them."),
                Untradable::NotConnected(_) => { let e = exchange_name(tx, id)?; format!("It has open orders on {e}, and trading on {e} isn't supported yet.") },
                Untradable::Unreadable(_) => "Its API key can't be read with this install's secret.".into(),
            };
            add(&mut left, id, said);
        }
    }
    let mut lines = vec![];
    for (id, said) in left { lines.push(format!("{}: {}", name(tx, id)?, said.join(" "))); }
    for id in &after.rules { lines.push(format!("Rule {id}: {}", rule_stopped(tx, *id)?)); }
    if !lines.is_empty() { return Ok(Err(Hard { lines, details })); }
    Ok(Ok(Plan { bots, rules, report: after }))
}

/// The reasons a set-aside bot carries (`KEY`), or None.
pub fn reasons(aside: Option<&serde_json::Value>) -> Option<Vec<String>> {
    let list = aside?.get("reasons")?.as_array()?;
    Some(list.iter().filter_map(|r| r.as_str().map(str::to_string)).collect())
}

/// C2 with its reasons, in `locale`.
pub fn message(locale: &str, reasons: &[String]) -> String {
    crate::web::i18n::text(locale, STOP_KEY, &[("reason", crate::web::i18n::Arg::Text(&reasons.join(" ")))])
}

/// `check`'s line for a bot `apply` sets aside.
pub fn would_stop(aside: &Aside) -> String {
    format!("{} will be stopped at the next start: {}", aside.name, aside.reasons.join(" "))
}

/// The start log's line for it.
pub fn stopped(aside: &Aside) -> String {
    format!("stopped {}: {}", aside.name, aside.reasons.join(" "))
}

/// C6: the hard stop, one line per bot. The details go to stderr first, beside the refusal, for support; the page shows
/// only C6.
pub fn hard_stop(hard: &Hard) -> String {
    for line in &hard.details { eprintln!("deltabadger: [check] {line}"); }
    let lines = &hard.lines;
    format!("Deltabadger didn't start: it can't safely stop these bots for you, because they have orders or data it can't account for yet:\n{}\nNothing was bought or sold. Contact support.",
            lines.join("\n"))
}

fn push_unique(list: &mut Vec<String>, s: String) { if !list.contains(&s) { list.push(s); } }

/// "Bot 7 (“My basket”)", or "Bot 7" without a label.
pub fn name(c: &Connection, id: i64) -> Result<String, EngineError> {
    let label: Option<String> = c.query_row("SELECT label FROM bots WHERE id = ?1", [id], |r| r.get(0)).optional()?.flatten();
    Ok(match label.filter(|l| !l.trim().is_empty() && l.trim() != format!("Bot {id}")) { Some(l) => format!("Bot {id} (“{}”)", l.trim()), None => format!("Bot {id}") })
}

fn exchange_name(c: &Connection, bot_id: i64) -> Result<String, EngineError> {
    let name: Option<String> = c.query_row("SELECT e.name FROM bots b JOIN exchanges e ON e.id = b.exchange_id WHERE b.id = ?1", [bot_id], |r| r.get(0)).optional()?.flatten();
    Ok(name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "its exchange".into()))
}

fn symbol(c: &Connection, asset: i64) -> Result<String, EngineError> {
    let s: Option<String> = c.query_row("SELECT symbol FROM assets WHERE id = ?1", [asset], |r| r.get(0)).optional()?.flatten();
    Ok(s.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "One of its assets".into()))
}

fn exchange(c: &Connection, bot_id: i64) -> Result<String, EngineError> {
    Ok(format!("Trading on {} isn't supported yet. Keep the bot stopped, or delete it.", exchange_name(c, bot_id)?))
}

fn no_key(exchange: &str) -> String { format!("There's no {exchange} API key. Add your key in Settings, then start the bot again.") }

/// C2r.
fn rule_stopped(c: &Connection, rule: i64) -> Result<String, EngineError> {
    let name: Option<String> = c.query_row("SELECT e.name FROM rules r JOIN exchanges e ON e.id = r.exchange_id WHERE r.id = ?1", [rule], |r| r.get(0)).optional()?.flatten();
    let name = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "this exchange".into());
    Ok(format!("Withdrawals from {name} aren't supported yet, so this rule was stopped while updating your data. Nothing was withdrawn."))
}

/// A switch's setting key in the owner's words ("the price drop condition"), or None for a key the pages never name.
fn feature(key: &str) -> Option<String> {
    let (selling, rest) = match key.strip_prefix("sell_") { Some(rest) => (true, rest), None => (false, key) };
    let name = match rest {
        "price_limited" => "price", "price_drop_limited" => "price drop", "moving_average_limited" => "moving average", "indicator_limited" => "indicator",
        "quote_amount_limited" if !selling => return Some("the amount limit".into()),
        "base_amount_limited" if !selling => return Some("the selling amount limit".into()),
        _ => return None,
    };
    Some(if selling { format!("the {name} condition for selling") } else { format!("the {name} condition") })
}

fn action(action: &str) -> &'static str {
    match action { "pause" => "pause the bot", "start_selling" => "start selling", "start_buying" => "start buying", "stop" => "stop the bot", _ => "do something else" }
}

const SETTINGS: &str = "Some of its settings are saved in a form that can't be read. Open the bot's settings, check them and save, then start the bot again. If that doesn't help, contact support.";

/// One reason in the owner's words: what is wrong, then what to do (b2-5 reason inventory, groups A–K).
pub fn sentence(c: &Connection, bot_id: i64, reason: &Reason) -> Result<String, EngineError> {
    let again = "then start the bot again.";
    Ok(match &reason.kind {
        Kind::Exchange => exchange(c, bot_id)?,
        Kind::Feature(Feature::Setting(key)) => match feature(key) {
            Some(f) => format!("It uses {f}, which isn't supported on this bot yet. Turn it off in the bot's settings, {again}"),
            None => format!("It uses a setting that isn't supported on this bot yet. Check its settings, {again}"),
        },
        Kind::Feature(Feature::Action { setting, action: a }) => format!("{} is set to {}, which isn't supported yet. Change what it does or turn it off, {again}",
            capital(&feature(setting).unwrap_or_else(|| "one of its conditions".into())), action(a)),
        Kind::Feature(Feature::ConditionCandles(key)) => format!("{} isn't available on {} yet. Turn it off in the bot's settings, {again}",
            capital(&feature(key).unwrap_or_else(|| "one of its conditions".into())), exchange_name(c, bot_id)?),
        Kind::Feature(Feature::IbkrLimit) => format!("Limit orders on Interactive Brokers aren't supported yet. Switch the bot to market orders, {again}"),
        Kind::Feature(Feature::IndexSelling) => format!("An index bot can't sell. Switch it back to buying, {again}"),
        Kind::Feature(Feature::BotType(_)) => "This kind of bot isn't supported yet. Keep it stopped.".into(),
        Kind::Flip { condition, to_selling, .. } => {
            let way = if *to_selling { "selling" } else { "buying" };
            format!("When {} is met it would start {way}, and it couldn't run that way. Check its {way} settings, or change what the condition does, {again}",
                    feature(condition).unwrap_or_else(|| "one of its conditions".into()))
        }
        Kind::Asset(Some(asset)) => format!("{} can't be traded by this bot on {}. Remove it from the bot or choose another exchange, {again}", symbol(c, *asset)?, exchange_name(c, bot_id)?),
        Kind::Asset(None) => format!("This bot's quote currency can't be used on {}. Choose another one, {again}", exchange_name(c, bot_id)?),
        Kind::Allocations(Allocations::Unbalanced(sum)) => match sum.parse::<f64>() {
            Ok(sum) => format!("Its allocations add up to {}%, not 100%. Adjust them, {again}", percent(sum)),
            Err(_) => format!("Its allocations don't add up to 100%. Adjust them, {again}"),
        },
        Kind::Allocations(Allocations::Empty) => format!("It has no assets. Add at least one, {again}"),
        Kind::Allocations(Allocations::TooMany(n)) => format!("It has {n} assets, and the most a bot can have is {}. Remove some, {again}", eligibility::MAX_ASSETS),
        Kind::Allocations(Allocations::Negative) => format!("One of its allocations is below zero. Adjust them, {again}"),
        Kind::Allocations(Allocations::Duplicate(asset)) => format!("{} is listed twice in its allocations. Remove one, {again}", symbol(c, *asset)?),
        Kind::Allocations(Allocations::Unreadable) | Kind::Settings | Kind::Owner => SETTINGS.into(),
        Kind::History(h) => {
            let what = match h {
                History::ImportedOpen => "imported orders that never closed", History::NoAsset => "orders without an asset",
                History::SpecialRows => "rebalancing, liquidation or redeploy orders", History::ForeignSells => "orders on another exchange",
                History::IndexAssets => "index assets",
            };
            format!("Its order history has {what} that can't be accounted for yet. Keep the bot stopped and contact support.")
        }
        Kind::IndexData => format!("Index bots need Deltabadger's market data, which isn't set up here. Set it up in Settings, {again}"),
        Kind::Work(w) => match w {
            Work::UnknownStatus => "It has orders in a state that can't be read.",
            Work::Pending => "It has a rebalance that didn't finish.",
            Work::Waiting => "It has rebalancing, liquidation or redeploy orders still open.",
            Work::Abandoned => "It has a rebalancing order whose fill is unknown.",
            Work::Rebalancing => "It still rebalances, in a way that isn't supported yet.",
            Work::Outstanding => "It has open orders.",
        }.into(),
    })
}

fn capital(s: &str) -> String { let mut c = s.chars(); c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect()) }

/// 0.5 → "50", 0.333 → "33.3".
fn percent(share: f64) -> String {
    let p = (share * 1000.0).round() / 10.0;
    if p.fract() == 0.0 { format!("{}", p as i64) } else { format!("{p}") }
}
