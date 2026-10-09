//! Bot::Restatable: what a bot knows about its share counts being restated. The broker's account of a split is
//! an `adjustment` ledger row marked `corporate_action: split`; this turns those rows into the events the walk
//! folds in.
use super::at::At;
use super::budget;
use std::collections::{HashMap, HashSet};
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

/// The split rows that apply to one holding on one effective date. No date: reports whose name stands for more than
/// one class of asset in the account, which restate nothing and leave the split unresolved.
pub struct Group { pub key: String, pub date: Option<i64>, pub rows: Vec<SplitRow> }

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
    let mut first = None;
    let mut conflict = false;
    for row in rows {
        budget::charge(1, 0)?;
        if let Some(f) = factor(&row.raw_data)? {
            match &first { Some(previous) => conflict |= *previous != f, None => first = Some(f) }
        }
    }
    Ok(if conflict { None } else { first })
}

/// Bot::Restatable#grouped_split_rows: the marked rows this bot is eligible for, per holding and effective date
/// (the UTC date of the row). Eligibility is the venues and symbols the bot actually traded. A report's asset is the
/// one its row recorded, else the one asset its name stands for on its venue. With both a holding's and a report's
/// asset known, only the same asset matches; otherwise the strings decide, and only while the string names one class
/// of asset in the user's account (`account_classes`) — a name that is both is grouped under no date.
pub fn groups(c: &Connection, user_id: i64, orders: &[Order], holdings: &[Holding]) -> Result<Vec<Group>, FiguresError> {
    let mut string_pairs = HashSet::new();
    let mut asset_pairs = HashSet::new();
    let mut exchange_ids = vec![];
    let mut exchanges_seen = HashSet::new();
    for order in orders {
        budget::charge(1, 0)?;
        let Some(exchange_id) = order.exchange_id else { continue };
        let base = order.base.as_deref().filter(|b| !b.trim().is_empty());
        if let Some(base) = base { string_pairs.insert((exchange_id, base)); }
        if let Some(asset_id) = order.asset_id { asset_pairs.insert((exchange_id, asset_id)); }
        if (base.is_some() || order.asset_id.is_some()) && exchanges_seen.insert(exchange_id) { exchange_ids.push(exchange_id); }
    }
    if exchange_ids.is_empty() { return Ok(vec![]); }
    budget::charge((exchange_ids.len() as u64).saturating_mul(u64::from(exchange_ids.len().ilog2()) + 1), 0)?;
    exchange_ids.sort_unstable();
    let rows = db::split_rows(c, user_id, &exchange_ids)?;
    if rows.is_empty() { return Ok(vec![]); }

    let mut wanted: HashMap<i64, Vec<String>> = HashMap::new();
    let mut seen = HashSet::new();
    for row in &rows {
        budget::charge(1, 0)?;
        if row.base_currency.trim().is_empty() { continue; }
        let name = row.base_currency.to_uppercase();
        if seen.insert((row.exchange_id, name.clone())) { wanted.entry(row.exchange_id).or_default().push(name); }
    }
    // None means that more than one asset claims this name on the venue.
    let mut named: HashMap<(i64, String), Option<i64>> = HashMap::new();
    for exchange_id in exchange_ids {
        budget::charge(1, 0)?;
        for (name, id) in db::asset_ids_by_name(c, exchange_id, wanted.get(&exchange_id).map_or(&[], Vec::as_slice))? {
            budget::charge(1, 0)?;
            named.entry((exchange_id, name)).and_modify(|found| { if *found != Some(id) { *found = None; } }).or_insert(Some(id));
        }
    }
    let mut holding_strings = Vec::new();
    for holding in holdings {
        budget::charge(1, 0)?;
        let mut strings = HashSet::new();
        for string in &holding.strings { budget::charge(1, 0)?; strings.insert(string.as_str()); }
        holding_strings.push(strings);
    }
    let mut symbols: Vec<String> = vec![];
    let mut symbols_seen: HashSet<String> = HashSet::new();
    for row in &rows { budget::charge(1, 0)?; if symbols_seen.insert(row.base_currency.clone()) { symbols.push(row.base_currency.clone()); } }
    let mut classes: HashMap<String, Vec<Option<String>>> = HashMap::new();
    for (symbol, category) in db::account_classes(c, user_id, &symbols)? { classes.entry(symbol).or_default().push(category); }
    let mut categories: HashMap<i64, Option<String>> = HashMap::new();
    let mut out: Vec<Group> = vec![];
    let mut places = HashMap::new();
    for row in rows {
        budget::charge(1, 0)?;
        let reported = row.base_asset_id.or_else(|| named.get(&(row.exchange_id, row.base_currency.to_uppercase())).copied().flatten());
        let day = row.at.0.div_euclid(86_400_000_000_000);
        for (holding, strings) in holdings.iter().zip(&holding_strings) {
            budget::charge(1, 0)?;
            let date = match (holding.asset_id, reported) {
                (Some(asset_id), Some(reported)) => {
                    if !(reported == asset_id && asset_pairs.contains(&(row.exchange_id, asset_id))) { continue; }
                    Some(day)
                }
                (asset_id, reported) => {
                    if !(strings.contains(row.base_currency.as_str()) && string_pairs.contains(&(row.exchange_id, row.base_currency.as_str()))) { continue; }
                    let mut named_classes: Vec<Option<String>> = classes.get(&row.base_currency).cloned().unwrap_or_default();
                    if let Some(id) = asset_id.or(reported) {
                        if let std::collections::hash_map::Entry::Vacant(e) = categories.entry(id) { e.insert(db::asset_category(c, id)?); }
                        named_classes.push(categories[&id].clone());
                    }
                    let mut distinct: Vec<String> = vec![];
                    for class in named_classes.into_iter().flatten() { if !distinct.contains(&class) { distinct.push(class); } }
                    (distinct.len() <= 1).then_some(day)
                }
            };
            let at = *places.entry((holding.key.clone(), date)).or_insert_with(|| {
                out.push(Group { key: holding.key.clone(), date, rows: vec![] }); out.len() - 1
            });
            out[at].rows.push(row.clone());
        }
    }
    budget::check()?;
    Ok(out)
}

/// Bot::Restatable#split_events: one event per holding and date whose reports agree on a factor, timed at the
/// earliest instant the group reports, already in effect at `now`, oldest first.
pub fn events(c: &Connection, user_id: i64, orders: &[Order], holdings: &[Holding], now: At) -> Result<Vec<Event>, FiguresError> {
    let mut out: Vec<Event> = vec![];
    for group in groups(c, user_id, orders, holdings)? {
        budget::charge(1 + group.rows.len() as u64, 0)?;
        if group.date.is_none() { continue; }
        let (Some(at), Some(factor)) = (group.rows.iter().map(|row| row.at).min(), resolved_factor(&group.rows)?) else { continue };
        if at <= now { out.push(Event { at, key: group.key, factor }); }
    }
    budget::charge((out.len() as u64).saturating_mul(u64::from(out.len().max(1).ilog2()) + 1), 0)?;
    out.sort_by(|a, b| (a.at, a.key.as_bytes()).cmp(&(b.at, b.key.as_bytes())));
    budget::check()?;
    Ok(out)
}

/// Bot::Restatable#unresolved_split?: a split in effect that this bot can see and cannot size, or whose name stands
/// for more than one class of asset in the account.
pub fn unresolved(c: &Connection, user_id: i64, orders: &[Order], holdings: &[Holding], now: At) -> Result<bool, FiguresError> {
    for group in groups(c, user_id, orders, holdings)? {
        budget::charge(1 + group.rows.len() as u64, 0)?;
        if group.rows.iter().map(|row| row.at).min().is_none_or(|at| at <= now) && (group.date.is_none() || resolved_factor(&group.rows)?.is_none()) { return Ok(true); }
    }
    Ok(false)
}
