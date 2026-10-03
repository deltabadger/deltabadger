//! Bot::Restatable: what a bot knows about its share counts being restated. The broker's account of a split is
//! an `adjustment` ledger row marked `corporate_action: split`; this turns those rows into the events the walk
//! folds in.
use super::at::At;
use super::db::{self, Order, SplitRow};
use super::dec::Dec;
use super::num::NumError;
use super::FiguresError;
use rusqlite::Connection;
use serde_json::Value;

/// How long a live price stays out of trust after a restatement (SPLIT_PRICE_QUARANTINE, two days).
pub const QUARANTINE_SECONDS: i64 = 2 * 86_400;

/// One holding of the walk: its key, its asset (none for rows recorded before orders stored theirs), and the
/// strings its rows were recorded under (Bot::Composition::Measurable#split_holdings).
#[derive(Clone, Debug)]
pub struct Holding { pub key: String, pub asset_id: Option<i64>, pub strings: Vec<String> }

/// What the holding under `key` is multiplied by, and when.
#[derive(Clone, Debug, PartialEq)]
pub struct Event { pub at: At, pub key: String, pub factor: Dec }

/// The split rows that apply to one holding on one effective date.
struct Group { key: String, rows: Vec<SplitRow> }

/// `"10:1"` is 10. Anything else names no factor: a blank, one number, a zero side, words, extra parts. A ratio of
/// numbers beyond `dec`'s limits is an error: Rails would restate by it.
pub fn factor(raw_data: &Value) -> Result<Option<Dec>, NumError> {
    let text = match raw_data.get("split_ratio") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    };
    let mut parts: Vec<&str> = text.split(':').collect();
    while parts.last() == Some(&"") { parts.pop(); } // String#split drops trailing empty parts
    let number = |part: &str| {
        let (whole, fraction) = part.split_once('.').map_or((part, None), |(w, f)| (w, Some(f)));
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        digits(whole) && fraction.is_none_or(digits)
    };
    if parts.len() != 2 || !parts.iter().all(|part| number(part)) { return Ok(None); }
    let (new_count, old_count) = (Dec::strict(parts[0])?, Dec::strict(parts[1])?);
    if !new_count.is_positive() || !old_count.is_positive() { return Ok(None); }
    new_count.div(&old_count).map(Some)
}

/// One factor, or none: a row that names no factor is silent, two rows naming different ones resolve to nothing.
fn resolved_factor(rows: &[SplitRow]) -> Result<Option<Dec>, NumError> {
    let mut factors: Vec<Dec> = vec![];
    for row in rows { if let Some(f) = factor(&row.raw_data)? { if !factors.contains(&f) { factors.push(f); } } }
    Ok(if factors.len() == 1 { factors.pop() } else { None })
}

/// Bot::Restatable#grouped_split_rows: the marked rows this bot is eligible for, per holding and effective date
/// (the UTC date of the row). Eligibility is the venues and symbols the bot actually traded.
fn groups(c: &Connection, user_id: i64, orders: &[Order], holdings: &[Holding]) -> Result<Vec<Group>, FiguresError> {
    let mut string_pairs: Vec<(i64, &str)> = vec![];
    let mut asset_pairs: Vec<(i64, i64)> = vec![];
    for order in orders {
        let Some(exchange_id) = order.exchange_id else { continue };
        if let Some(base) = order.base.as_deref().filter(|b| !b.trim().is_empty()) {
            if !string_pairs.contains(&(exchange_id, base)) { string_pairs.push((exchange_id, base)); }
        }
        if let Some(asset_id) = order.asset_id {
            if !asset_pairs.contains(&(exchange_id, asset_id)) { asset_pairs.push((exchange_id, asset_id)); }
        }
    }
    if string_pairs.is_empty() && asset_pairs.is_empty() { return Ok(vec![]); }
    let mut exchange_ids: Vec<i64> = string_pairs.iter().map(|p| p.0).chain(asset_pairs.iter().map(|p| p.0)).collect();
    exchange_ids.sort_unstable();
    exchange_ids.dedup();
    let rows: Vec<SplitRow> = db::split_rows(c, user_id, &exchange_ids)?.into_iter()
        .filter(|row| row.raw_data.get("corporate_action").and_then(Value::as_str) == Some("split")).collect();
    if rows.is_empty() { return Ok(vec![]); }

    // The one asset each report's name stands for on its venue, by spelling or symbol; none when it names none or several.
    let mut named: Vec<(i64, Vec<(String, i64)>)> = vec![];
    for &exchange_id in &exchange_ids {
        let mut wanted: Vec<String> = vec![];
        for row in rows.iter().filter(|row| row.exchange_id == exchange_id && !row.base_currency.trim().is_empty()) {
            let name = row.base_currency.to_uppercase();
            if !wanted.contains(&name) { wanted.push(name); }
        }
        named.push((exchange_id, db::asset_ids_by_name(c, exchange_id, &wanted)?));
    }
    let report_asset = |row: &SplitRow| -> Option<i64> {
        let name = row.base_currency.to_uppercase();
        let found = &named.iter().find(|(exchange_id, _)| *exchange_id == row.exchange_id)?.1;
        let mut ids = found.iter().filter(|(n, _)| *n == name).map(|(_, id)| *id);
        match (ids.next(), ids.next()) { (Some(id), None) => Some(id), _ => None }
    };

    let mut out: Vec<(String, i64, Group)> = vec![]; // (key, days since the epoch, group)
    for row in rows {
        let reported = report_asset(&row);
        let date = row.at.0.div_euclid(86_400_000_000_000);
        for holding in holdings {
            let applies = match (holding.asset_id, reported) {
                (Some(asset_id), Some(reported)) => reported == asset_id && asset_pairs.contains(&(row.exchange_id, asset_id)),
                _ => holding.strings.contains(&row.base_currency) && string_pairs.contains(&(row.exchange_id, row.base_currency.as_str())),
            };
            if !applies { continue; }
            match out.iter_mut().find(|(key, day, _)| *key == holding.key && *day == date) {
                Some((_, _, group)) => group.rows.push(row.clone()),
                None => out.push((holding.key.clone(), date, Group { key: holding.key.clone(), rows: vec![row.clone()] })),
            }
        }
    }
    Ok(out.into_iter().map(|(_, _, group)| group).collect())
}

/// Bot::Restatable#split_events: one event per holding and date whose reports agree on a factor, timed at the
/// earliest instant the group reports, already in effect at `now`, oldest first.
pub fn events(c: &Connection, user_id: i64, orders: &[Order], holdings: &[Holding], now: At) -> Result<Vec<Event>, FiguresError> {
    let mut out: Vec<Event> = vec![];
    for group in groups(c, user_id, orders, holdings)? {
        let (Some(at), Some(factor)) = (group.rows.iter().map(|row| row.at).min(), resolved_factor(&group.rows)?) else { continue };
        if at <= now { out.push(Event { at, key: group.key, factor }); }
    }
    out.sort_by(|a, b| (a.at, a.key.as_bytes()).cmp(&(b.at, b.key.as_bytes())));
    Ok(out)
}

/// Bot::Restatable#unresolved_split?: a split in effect that this bot can see and cannot size.
pub fn unresolved(c: &Connection, user_id: i64, orders: &[Order], holdings: &[Holding], now: At) -> Result<bool, FiguresError> {
    for group in groups(c, user_id, orders, holdings)? {
        if group.rows.iter().map(|row| row.at).min().is_none_or(|at| at <= now) && resolved_factor(&group.rows)?.is_none() { return Ok(true); }
    }
    Ok(false)
}
