//! PLAN index-as-ingredient slice 6 (D-INDEX-JOINS-BASKET): an index picked on the Assets step joins the selection beside the
//! assets and the other indexes instead of replacing them.
//!
//! The session holds a selection in one of two forms. **Lone** (no asset, one index): the index wizard's own top-level keys,
//! exactly as before, so its steps, gates and save (`Bots::DcaIndex`) are untouched. **Mixed** (anything else holding an
//! index): `base_asset_ids` beside `index_ids`, the picked `indices` row ids (integers: a hundred ingredients stay well inside
//! the session cookie); names and members are read from the rows. `write_selection` is the one writer of both forms.
//!
//! A mixed selection goes the basket's way (the multi namespace) and is saved as one grouped `Bots::DcaMultiAsset`. Every
//! decision on it (the venues listed and posted, the key step, the broker shortcut, the quotes listed, the save) asks
//! `eligible`. The lone index keeps the index wizard's own, weaker gates: the explicit exception the plan names.
use super::index::{self, Pill};
use super::{array, basket_ids, dig, index_chosen, to_i, INDEX_IDS_KEY};
use crate::web::layout::Ctx;
use crate::web::{App, WebError};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Map, Value};

/// A mixed selection's index rows, in pick order, without blanks and repeats.
pub fn mixed_rows(config: Option<&Value>) -> Vec<i64> {
    let mut rows: Vec<i64> = vec![];
    for row in array(dig(config, INDEX_IDS_KEY)).iter().map(to_i).filter(|row| *row > 0) {
        if !rows.contains(&row) { rows.push(row); }
    }
    rows
}

/// The row a lone index stands for: the first `indices` row of the index it follows the picker shows (`index::tiles`: a
/// data-api row only on a data-api install), in its order.
fn lone_row(c: &Connection, app: &App, config: Option<&Value>) -> Result<Option<i64>, WebError> {
    if !index_chosen(config) { return Ok(None) }
    let followed = index::followed(config.and_then(|config| config.get("settings")).and_then(Value::as_object));
    let deltabadger = crate::web::bots::market_data(c, app)?.0;
    Ok(c.query_row("SELECT id FROM indices WHERE external_id = ?1 AND (?2 OR source != 'deltabadger') \
                    ORDER BY CASE source WHEN 'deltabadger' THEN 0 WHEN 'internal' THEN 1 ELSE 2 END, weight DESC, market_cap DESC, id LIMIT 1",
                   rusqlite::params![followed, deltabadger], |r| r.get(0)).optional()?)
}

/// Every index the selection holds, in pick order: the mixed rows, else the lone index's row.
pub fn held_rows(c: &Connection, app: &App, config: Option<&Value>) -> Result<Vec<i64>, WebError> {
    let rows = mixed_rows(config);
    if !rows.is_empty() { return Ok(rows) }
    Ok(lone_row(c, app, config)?.into_iter().collect())
}

/// The external ids of the held indexes whose row still exists (what the Index view leaves out).
pub fn held_external_ids(c: &Connection, app: &App, config: Option<&Value>) -> Result<Vec<String>, WebError> {
    let mut ids = vec![];
    for row in held_rows(c, app, config)? {
        if let Some(id) = c.query_row("SELECT external_id FROM indices WHERE id = ?1", [row], |r| r.get::<_, Option<String>>(0)).optional()?.flatten() { ids.push(id); }
    }
    Ok(ids)
}

/// A held index as the track's stack and the chosen list's row draw it: its row id (what its × posts), its name, its first
/// members. A row no longer in the catalogue is named "Index", with no members, and stays removable.
pub struct Held { pub row: i64, pub name: String, pub pills: Vec<Pill> }

fn held_of(c: &Connection, ctx: &Ctx, rows: &[i64]) -> Result<Vec<Held>, WebError> {
    if rows.is_empty() { return Ok(vec![]) }
    let mut tiles = index::tiles(c, &ctx.app, None)?;
    Ok(rows.iter().map(|row| match tiles.iter().position(|tile| tile.row_id == *row) {
        Some(at) => { let tile = tiles.swap_remove(at); Held { row: *row, name: tile.name, pills: tile.pills } }
        None => Held { row: *row, name: ctx.t("bot.setup.kind.index"), pills: vec![] },
    }).collect())
}

/// The chosen list's index rows: every held index, the lone one too.
pub fn held(c: &Connection, ctx: &Ctx, config: Option<&Value>) -> Result<Vec<Held>, WebError> { held_of(c, ctx, &held_rows(c, &ctx.app, config)?) }

/// The track's stacks: a mixed selection's indexes (a lone index has the index wizard's own track).
pub fn stacks(c: &Connection, ctx: &Ctx, config: Option<&Value>) -> Result<Vec<Held>, WebError> { held_of(c, ctx, &mixed_rows(config)) }

/// The lone index's keys, as the Index view's pick writes them: Top as four nulls with its type, a category by id, the name
/// its row shows and the count prefix.
pub fn lone_keys(settings: &mut Map<String, Value>, category: Option<&str>, name: Option<String>) {
    match category {
        Some(id) => {
            let prefix = super::index_create::count_named(Some(id)).map(|(prefix, _)| prefix);
            settings.insert("index_type".into(), json!("category"));
            settings.insert("index_category_id".into(), json!(id));
            settings.insert("index_name".into(), json!(name));
            settings.insert("index_name_prefix".into(), json!(prefix));
        }
        None => {
            for key in ["index_type", "index_category_id", "index_name", "index_name_prefix"] { settings.insert(key.into(), Value::Null); }
            settings.insert("index_type".into(), json!("top"));
        }
    }
}

/// The one writer of a selection: every wizard key but the exchange goes, a lone index's keys too, then the canonical form of
/// `ids` and `rows`. One index and no asset is the lone form, written from its row (one no longer in the catalogue stays in
/// the mixed form, so its × is still drawn); anything else the mixed form. An empty list is not written.
pub fn write_selection(c: &Connection, config: &mut Value, ids: &[i64], rows: &[i64]) -> Result<(), WebError> {
    if !config.is_object() { *config = json!({}); }
    super::reset_keys_but_exchange(config);
    for key in super::INDEX_KEYS { super::delete_path(config, key); }
    if ids.is_empty() && rows.is_empty() { return Ok(()) }
    let lone: Option<Option<Texts3>> = match (ids, rows) {
        ([], [row]) => Some(c.query_row("SELECT external_id, source, name FROM indices WHERE id = ?1 AND external_id IS NOT NULL", [row], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?),
        _ => None,
    };
    if !config["settings"].is_object() { config["settings"] = json!({}); }
    let settings = config["settings"].as_object_mut().expect("an object, made one above");
    match lone {
        // The name the Index view's row shows (index::tiles), which its pick posts.
        Some(Some((external_id, source, name))) => {
            let shown = super::index_create::count_named(Some(&external_id)).map_or_else(|| name.unwrap_or_default(), |(_, named)| named.to_string());
            lone_keys(settings, (source.as_deref() != Some("internal")).then_some(external_id.as_str()), Some(shown))
        }
        // A row the catalogue no longer has stays a pick, drawn as "Index" and removable; its save is refused.
        Some(None) | None => {
            if !ids.is_empty() { settings.insert("base_asset_ids".into(), json!(ids)); }
            if !rows.is_empty() { settings.insert(INDEX_IDS_KEY[1].into(), json!(rows)); }
        }
    }
    Ok(())
}

/// What a quote form drawn for a mixed selection carries (`selection=`), so a form drawn before the selection or the venue
/// changed saves nothing (a pick that binds no account names no venue in the form's URL).
pub fn digest(config: Option<&Value>) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(json!([super::exchange_id(config), basket_ids(config), mixed_rows(config)]).to_string()))[..16].to_string()
}

/// Text columns of an `indices` row as `write_selection` and `eligible` read them.
type Texts3 = (String, Option<String>, Option<String>);
type Texts4 = (Option<String>, Option<String>, Option<String>, Option<String>);

/// Bots::Index MINIMUM_SUPPORTED_COINS.
const MINIMUM_MEMBERS: usize = 3;

/// The one eligibility check of a grouped create: whether a bot of `assets` and the index `rows` could be made on `exchange`
/// (at `quote`, or at some quote the venue lists). It requires an available venue this engine runs index bots on
/// (`index::refusal`) with a ranking provider the engine can read, each row still in the catalogue, not retired, naming the venue (Top, no venue named, or the venue's
/// type); and one quote, USD on Alpaca, at which every asset trades and each index has at least three distinct
/// non-stablecoin members trading, counted on the ranking the engine derives from (`ranked_row`; the picked row's own list
/// while none is ranked).
pub fn eligible(c: &Connection, app: &App, exchange: i64, quote: Option<i64>, assets: &[i64], rows: &[i64]) -> Result<bool, WebError> {
    use crate::engine::index as engine;
    let venue: Option<(Option<String>, bool)> = c.query_row("SELECT type, available FROM exchanges WHERE id = ?1", [exchange], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    let Some((Some(class), true)) = venue else { return Ok(false) };
    // The engine composes the groups: a ranking provider it can read (a selected provider without its credentials is none).
    if crate::web::bot::retired_type(&class) || index::refusal(c, app, &class)?.is_some() || !crate::engine::eligibility::index_data(c)? { return Ok(false) }
    let mut rankings: Vec<String> = vec![];
    for row in rows {
        let found: Option<Texts4> = c.query_row(
            "SELECT external_id, source, top_coins, available_exchanges FROM indices WHERE id = ?1", [row], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional()?;
        let Some((Some(external_id), source, top_coins, venues)) = found else { return Ok(false) };
        if engine::retired(c, &external_id)? { return Ok(false) }
        let named: Vec<String> = venues.and_then(|text| serde_json::from_str::<Value>(&text).ok()).and_then(|v| v.as_object().map(|o| o.keys().cloned().collect())).unwrap_or_default();
        if source.as_deref() != Some("internal") && !named.is_empty() && !named.contains(&class) { return Ok(false) }
        rankings.push(engine::ranked_row(c, &external_id)?.map_or(top_coins, |ranked| ranked.top_coins).unwrap_or_else(|| "[]".into()));
    }
    let usd = super::usd_asset(c)?;
    let quotes: Vec<i64> = match quote {
        Some(quote) => vec![quote],
        None => {
            let mut s = c.prepare("SELECT DISTINCT quote_asset_id FROM tickers WHERE exchange_id = ?1 AND available = 1 AND trading_enabled = 1 AND quote_asset_id IS NOT NULL ORDER BY 1")?;
            let quotes = s.query_map([exchange], |r| r.get(0))?.collect::<Result<_, _>>()?;
            quotes
        }
    };
    let mut unique: Vec<i64> = vec![];
    for id in assets { if !unique.contains(id) { unique.push(*id); } }
    // ponytail: one count per (quote, ingredient); a venue listing hundreds of quotes would want one grouped query.
    'quote: for quote in quotes {
        if class == "Exchanges::Alpaca" && Some(quote) != usd { continue }
        let traded: i64 = c.query_row("SELECT COUNT(DISTINCT base_asset_id) FROM tickers WHERE exchange_id = ?1 AND quote_asset_id = ?2 AND available = 1 AND trading_enabled = 1 \
                                       AND base_asset_id IN (SELECT value FROM json_each(?3))", rusqlite::params![exchange, quote, json!(unique).to_string()], |r| r.get(0))?;
        if traded != unique.len() as i64 { continue }
        for ranking in &rankings {
            let mut s = c.prepare_cached("SELECT DISTINCT a.external_id, a.category FROM assets a JOIN tickers t ON t.base_asset_id = a.id \
                                          WHERE a.external_id IN (SELECT value FROM json_each(?1)) AND t.exchange_id = ?2 AND t.quote_asset_id = ?3 AND t.available = 1 AND t.trading_enabled = 1")?;
            let mut members: Vec<String> = vec![];
            for row in s.query_map(rusqlite::params![ranking, exchange, quote], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))? {
                let (id, category) = row?;
                if !engine::stablecoin(category.as_deref(), &id) && !members.contains(&id) { members.push(id); }
            }
            if members.len() < MINIMUM_MEMBERS { continue 'quote }
        }
        return Ok(true);
    }
    Ok(false)
}

/// `eligible` for the session's selection.
pub fn session_eligible(c: &Connection, app: &App, config: Option<&Value>, exchange: i64, quote: Option<i64>) -> Result<bool, WebError> {
    eligible(c, app, exchange, quote, &basket_ids(config), &mixed_rows(config))
}

/// Whether the session holds a mixed selection.
pub fn mixed(config: Option<&Value>) -> bool { !mixed_rows(config).is_empty() }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_rows_read_ids_in_pick_order_once() {
        assert_eq!(mixed_rows(Some(&json!({"settings": {"index_ids": [3, "2", 3, 0, null]}}))), vec![3, 2]);
        assert!(mixed_rows(Some(&json!({"settings": {"index_type": "top"}}))).is_empty(), "a lone index is not mixed");
    }

    #[test]
    fn the_digest_follows_the_selection() {
        let a = json!({"settings": {"base_asset_ids": [16], "index_ids": [2]}});
        let b = json!({"settings": {"base_asset_ids": [16]}});
        assert_ne!(digest(Some(&a)), digest(Some(&b)));
        assert_eq!(digest(Some(&a)), digest(Some(&json!({"settings": {"base_asset_ids": ["16"], "index_ids": [2]}}))));
        // A form drawn on one venue saves nothing on another (a pick that binds no account names none in its URL).
        let on = |venue: &str| digest(Some(&json!({"exchange_id": venue, "settings": {"base_asset_ids": [16], "index_ids": [2]}})));
        assert_ne!(on("3"), on("4"));
    }

    #[test]
    fn removing_down_to_one_index_restores_the_lone_keys() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE indices (id INTEGER PRIMARY KEY, external_id TEXT, source TEXT, name TEXT);
                         INSERT INTO indices VALUES (2, 'top-coins', 'internal', 'Top Coins'), (3, 'layer-1', 'coingecko', 'Layer 1');").unwrap();
        let mut config = json!({"exchange_id": "3", "settings": {"base_asset_ids": [16], "index_ids": [2, 3]}});
        write_selection(&c, &mut config, &[], &[3]).unwrap();
        assert_eq!(config, json!({"exchange_id": "3", "settings": {"index_type": "category", "index_category_id": "layer-1", "index_name": "Layer 1", "index_name_prefix": null}}));
        write_selection(&c, &mut config, &[], &[2]).unwrap();
        assert_eq!(config["settings"], json!({"index_type": "top", "index_category_id": null, "index_name": null, "index_name_prefix": null}));
        // Back to mixed, then a row the catalogue no longer has left alone: it stays a pick (drawn as "Index", removable).
        write_selection(&c, &mut config, &[16], &[2]).unwrap();
        assert_eq!(config["settings"], json!({"base_asset_ids": [16], "index_ids": [2]}));
        write_selection(&c, &mut config, &[], &[9]).unwrap();
        assert_eq!(config["settings"], json!({"index_ids": [9]}));
    }

    #[test]
    fn one_index_and_no_asset_is_stored_as_today() {
        let mut settings = Map::new();
        lone_keys(&mut settings, None, Some("ignored".into()));
        assert_eq!(Value::Object(settings), json!({"index_type": "top", "index_category_id": null, "index_name": null, "index_name_prefix": null}));
        let mut settings = Map::new();
        lone_keys(&mut settings, Some("layer-1"), Some("Layer 1".into()));
        assert_eq!(Value::Object(settings), json!({"index_type": "category", "index_category_id": "layer-1", "index_name": "Layer 1", "index_name_prefix": null}));
    }
}
