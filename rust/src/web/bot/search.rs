//! The asset search modal (Bots::AssetSearchesController#edit with Bots::Searchable): the assets a basket can add, searched in
//! four phases and shown in pages of 20 behind lazy frames. The search itself takes a `Basket`, not a bot, so the wizard can
//! reuse it with what its session holds.
use super::page::{self, Found};
use super::{exchange_svg, For, Kind};
use crate::web::{auth, colors, layout::{self, Ctx, Page}, shell::{self, Shell}, App, WebError};
use askama::Template;
use axum::{extract::{Extension, Path, State}, http::{header, StatusCode}, response::{IntoResponse, Response}};
use rusqlite::{params, Connection, OptionalExtension};

/// Bots::Searchable::ASSET_PAGE_SIZE.
pub const PAGE_SIZE: usize = 20;

/// What Bots::DcaMultiAsset#offered_tickers reads: its venue, its quote and its members.
#[derive(Clone, Debug, Default)]
pub struct Basket {
    pub exchange_id: Option<i64>,
    pub quote_asset_id: Option<i64>,
    pub base_asset_ids: Vec<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side { Base, Quote }

impl Side {
    fn column(self) -> &'static str { if self == Side::Base { "base_asset_id" } else { "quote_asset_id" } }
}

/// One result row: `pluck(:id, :symbol, :name, :color, :category, :image_url)` and, on a page, its exchanges (name_id, name).
#[derive(Clone, Debug)]
pub struct Row {
    pub id: i64,
    pub symbol: String,
    pub name: Option<String>,
    pub color: Option<String>,
    pub category: Option<String>,
    pub image_url: Option<String>,
    pub market_cap_rank: Option<i64>,
    pub exchanges: Vec<(String, String)>,
}

/// Bots::DcaMultiAsset#offered_tickers as SQL over ?1 venue, ?2 quote, ?3 members (JSON), ?4 their count, ?5 what a base must not be.
fn offered(side: Side) -> String {
    let filter = match side {
        Side::Base => "t.base_asset_id NOT IN (SELECT value FROM json_each(?5))",
        // ?5 is empty here: named only so both sides bind the same five values.
        Side::Quote => "(?4 = 0 OR t.base_asset_id IN (SELECT value FROM json_each(?3))) AND t.base_asset_id NOT IN (SELECT value FROM json_each(?5))",
    };
    format!("SELECT t.* FROM tickers t WHERE t.available = 1 AND t.trading_enabled = 1 AND {filter} AND (t.exchange_id, t.quote_asset_id) IN ({})", ELIGIBLE_PAIRS)
}

/// Bots::DcaMultiAsset#eligible_pairs over ?1 venue, ?2 quote, ?3 members (JSON), ?4 their count: the (venue, quote) pairs listing
/// every member; Exchange.tradeable is available and not retired.
const ELIGIBLE_PAIRS: &str = "SELECT p.exchange_id, p.quote_asset_id FROM tickers p JOIN exchanges e ON e.id = p.exchange_id \
     WHERE p.available = 1 AND p.trading_enabled = 1 AND e.available = 1 AND e.type IS NOT 'Exchanges::Bitmart' AND (?1 IS NULL OR e.id = ?1) \
     AND (?2 IS NULL OR p.quote_asset_id = ?2) AND (?4 = 0 OR p.base_asset_id IN (SELECT value FROM json_each(?3))) \
     GROUP BY p.exchange_id, p.quote_asset_id HAVING ?4 = 0 OR COUNT(DISTINCT p.base_asset_id) = ?4";

/// Bots::DcaMultiAsset#available_exchanges_for_current_settings (every tradeable venue, the basket's own venue not narrowing it):
/// the venues with an eligible pair, by id. With `single`, Bot::AssetConfigurable's one-pair version for the wizard's blank
/// one-asset bot: any available venue (retired ones included) with a tradeable ticker in the quote and of the base it holds.
pub fn available_exchanges(c: &Connection, basket: &Basket, single: Option<Option<i64>>) -> Result<Vec<super::Exchange>, WebError> {
    let (_, quote, members, count, _) = offered_params(basket, Side::Base);
    let row = |r: &rusqlite::Row| Ok(super::Exchange {
        id: r.get(0)?, name: r.get::<_, Option<String>>(1)?.unwrap_or_default(), class: r.get::<_, Option<String>>(2)?.unwrap_or_default(), maker_fee: r.get(3)?,
    });
    let found = match single {
        Some(base) => c.prepare("SELECT id, name, type, maker_fee FROM exchanges WHERE available = 1 AND id IN (SELECT exchange_id FROM tickers \
                                 WHERE available = 1 AND trading_enabled = 1 AND (?1 IS NULL OR quote_asset_id = ?1) AND (?2 IS NULL OR base_asset_id = ?2)) ORDER BY id")?
            .query_map(params![quote, base], row)?.collect::<Result<Vec<_>, _>>()?,
        None => c.prepare(&format!("SELECT id, name, type, maker_fee FROM exchanges WHERE id IN (SELECT exchange_id FROM ({ELIGIBLE_PAIRS})) ORDER BY id"))?
            .query_map(params![Option::<i64>::None, quote, members, count], row)?.collect::<Result<Vec<_>, _>>()?,
    };
    Ok(found)
}

fn offered_params(basket: &Basket, side: Side) -> (Option<i64>, Option<i64>, String, i64, String) {
    let excluded: Vec<i64> = if side == Side::Base { basket.base_asset_ids.iter().copied().chain(basket.quote_asset_id).collect() } else { vec![] };
    (basket.exchange_id, basket.quote_asset_id, serde_json::json!(basket.base_asset_ids).to_string(), basket.base_asset_ids.len() as i64,
     serde_json::json!(excluded).to_string())
}

/// `String#downcase`: full Unicode mapping, character by character (no final sigma).
pub fn downcase(text: &str) -> String { text.chars().flat_map(char::to_lowercase).collect() }

fn blank(text: &str) -> bool { text.trim().is_empty() }

fn jaro(a: &[char], b: &[char]) -> f64 {
    if a.is_empty() || b.is_empty() { return 0.0; }
    let (short, long) = if a.len() > b.len() { (b, a) } else { (a, b) };
    let window = (long.len() / 2).saturating_sub(1);
    let (mut short_hit, mut long_hit) = (vec![false; short.len()], vec![false; long.len()]);
    let mut matches = 0usize;
    for (i, c) in short.iter().enumerate() {
        for j in i.saturating_sub(window)..=(i + window).min(long.len() - 1) {
            if !long_hit[j] && *c == long[j] {
                (short_hit[i], long_hit[j]) = (true, true);
                matches += 1;
                break;
            }
        }
    }
    if matches == 0 { return 0.0; }
    let (mut transpositions, mut k) = (0usize, 0usize);
    for c in short.iter().zip(&short_hit).filter(|(_, hit)| **hit).map(|(c, _)| c) {
        let j = (k..long.len()).find(|j| long_hit[*j]).unwrap_or(long.len() - 1);
        k = j + 1;
        if *c != long[j] { transpositions += 1; }
    }
    let (m, t) = (matches as f64, (transpositions / 2) as f64);
    (m / short.len() as f64 + m / long.len() as f64 + (m - t) / m) / 3.0
}

/// `JaroWinkler.similarity` with the jaro_winkler gem's defaults: weight 0.1, threshold 0.7, case kept, no adjusting table.
pub fn jaro_winkler(a: &str, b: &str) -> f64 {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let distance = jaro(&a, &b);
    if distance < 0.7 { return distance; }
    let prefix = a.iter().zip(&b).take(4).take_while(|(x, y)| x == y).count();
    // As the gem's C is built on this Mac (clang contracts `j + p * w * (1 - j)` into one fused multiply-add), which the recorded
    // vectors pin. ponytail: a build without FMA (gcc on x86-64) rounds the last bit otherwise; it matters only exactly at a cut-off.
    (prefix as f64 * 0.1).mul_add(1.0 - distance, distance)
}

/// `CGI.escape`.
pub(crate) fn cgi_escape(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => out.push(byte as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn decode(text: &str) -> String {
    form_urlencoded::parse(format!("x={text}").as_bytes()).next().map(|(_, value)| value.into_owned()).unwrap_or_default()
}

/// `request.query_parameters.merge('offset' => offset).to_query` for a raw query string: Rack's parse (the last of a repeated
/// name wins, `name[]` collects, `name[key]` nests one level), then Hash#to_query, which sorts the encoded pairs.
/// ponytail: deeper nesting and Rack's 400 on malformed keys are not modelled; nothing the search links to sends them.
pub fn next_page_query(raw: &str, offset: i64) -> String {
    enum Value { One(Option<String>), List(Vec<Option<String>>), Map(Vec<(String, Option<String>)>) }
    let mut params: Vec<(String, Value)> = vec![];
    for piece in raw.split('&').filter(|piece| !piece.is_empty()) {
        let (key, value) = piece.split_once('=').map_or((piece, None), |(k, v)| (k, Some(decode(v))));
        let key = decode(key);
        if key.is_empty() { continue; }
        let (name, nested) = key.split_once('[').map_or((key.as_str(), None), |(name, rest)| (name, Some(rest.trim_end_matches(']'))));
        let slot = match params.iter().position(|(existing, _)| existing == name) {
            Some(at) => at,
            None => { params.push((name.to_string(), Value::One(None))); params.len() - 1 }
        };
        let entry = &mut params[slot].1;
        match nested {
            None => *entry = Value::One(value),
            Some("") => match entry { Value::List(list) => list.push(value), _ => *entry = Value::List(vec![value]) },
            Some(inner) => {
                if !matches!(entry, Value::Map(_)) { *entry = Value::Map(vec![]); }
                if let Value::Map(map) = entry {
                    map.retain(|(existing, _)| existing != inner);
                    map.push((inner.to_string(), value));
                }
            }
        }
    }
    params.retain(|(name, _)| name != "offset");
    params.push(("offset".into(), Value::One(Some(offset.to_string()))));
    let pair = |key: &str, value: &Option<String>| match value {
        Some(value) => format!("{}={}", cgi_escape(key), cgi_escape(value)),
        None => cgi_escape(key),
    };
    let mut encoded: Vec<String> = params.iter().filter_map(|(name, value)| match value {
        Value::One(value) => Some(pair(name, value)),
        Value::List(list) => Some(list.iter().map(|value| pair(&format!("{name}[]"), value)).collect::<Vec<_>>().join("&")),
        Value::Map(map) if map.is_empty() => None,
        Value::Map(map) => {
            let mut inner: Vec<String> = map.iter().map(|(key, value)| pair(&format!("{name}[{key}]"), value)).collect();
            inner.sort();
            Some(inner.join("&"))
        }
    }).collect();
    encoded.sort();
    encoded.join("&")
}

/// `String#to_i`: leading whitespace, a sign, digits (single underscores between them); 0 when there are none.
pub(crate) fn to_i(text: &str) -> i64 {
    let text = text.trim_start();
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let mut value: i64 = 0;
    let mut previous_underscore = false;
    for (index, c) in digits.chars().enumerate() {
        match c {
            '0'..='9' => { value = value.saturating_mul(10).saturating_add(i64::from(c as u8 - b'0')); previous_underscore = false; }
            '_' if index > 0 && !previous_underscore => previous_underscore = true,
            _ => break,
        }
    }
    if negative { -value } else { value }
}

fn rows(c: &Connection, sql: &str, args: impl rusqlite::Params) -> Result<Vec<Row>, WebError> {
    let mut statement = c.prepare(sql)?;
    let found = statement.query_map(args, |r| Ok(Row {
        id: r.get(0)?, symbol: r.get::<_, Option<String>>(1)?.unwrap_or_default(), name: r.get(2)?, color: r.get(3)?, category: r.get(4)?,
        image_url: r.get(5)?, market_cap_rank: r.get(6)?, exchanges: vec![],
    }))?.collect::<Result<Vec<_>, _>>()?;
    Ok(found)
}

/// Bots::Searchable#filtered_asset_rows: every asset the basket offers on that side, ranked for the query, without exchanges.
pub fn filtered_rows(c: &Connection, basket: &Basket, side: Side, query: Option<&str>) -> Result<Vec<Row>, WebError> {
    let (venue, quote, members, count, excluded) = offered_params(basket, side);
    let available = format!("id IN (SELECT DISTINCT {} FROM ({}))", side.column(), offered(side));
    let select = "SELECT id, symbol, name, color, category, image_url, market_cap_rank FROM assets WHERE";
    let ranked = "ORDER BY market_cap_rank IS NULL, market_cap_rank, id";
    let found = match query.filter(|query| !blank(query)) {
        None => rows(c, &format!("{select} {available} ORDER BY market_cap_rank IS NULL, market_cap_rank, symbol, id"), params![venue, quote, members, count, excluded])?,
        Some(query) => {
            let q = downcase(query);
            use rusqlite::types::Value as Sql;
            let mut args: Vec<Sql> = vec![venue.into(), quote.into(), members.into(), count.into(), excluded.into(), q.clone().into(),
                                          format!("{q}%").into(), format!("%{q}%").into()];
            // Each statement binds ?1 up to the highest it names.
            let upto = |n: usize, args: &[Sql]| rusqlite::params_from_iter(args[..n].to_vec());
            let exact = "SELECT id FROM assets WHERE LOWER(symbol) = ?6";
            let prefix = format!("SELECT id FROM assets WHERE LOWER(symbol) LIKE ?7 AND id NOT IN ({exact})");
            let mut found = rows(c, &format!("{select} {available} AND LOWER(symbol) = ?6 {ranked}"), upto(6, &args))?;
            found.extend(rows(c, &format!("{select} {available} AND LOWER(symbol) LIKE ?7 AND id NOT IN ({exact}) {ranked}"), upto(7, &args))?);
            found.extend(rows(c, &format!("{select} {available} AND LOWER(name) LIKE ?8 AND id NOT IN ({exact}) AND id NOT IN ({prefix}) {ranked}"), upto(8, &args))?);
            // Phase 4: the first 2000 others in rowid order (Rails' LIMIT without ORDER BY), kept when symbol or name is 0.85 alike.
            args.push(serde_json::json!(found.iter().map(|row| row.id).collect::<Vec<_>>()).to_string().into());
            let candidates = rows(c, &format!("{select} {available} AND id NOT IN (SELECT value FROM json_each(?9)) ORDER BY id LIMIT 2000"), upto(9, &args))?;
            let score = |text: &Option<String>| text.as_deref().filter(|text| !blank(text)).map_or(0.0, |text| jaro_winkler(&downcase(text), &q));
            let mut fuzzy: Vec<Row> = candidates.into_iter().filter(|row| score(&Some(row.symbol.clone())).max(score(&row.name)) >= 0.85).collect();
            // ponytail: a stable sort; Ruby's sort_by is not, so 17 or more fuzzy matches tied on rank may come out in another order.
            fuzzy.sort_by_key(|row| row.market_cap_rank.map_or(i128::MAX, i128::from));
            found.extend(fuzzy);
            found
        }
    };
    Ok(found.into_iter().filter(|row| !blank(&row.symbol)).collect())
}

/// `basket.available_assets_for_current_settings(asset_type: side).exists?(id)`: whether the basket offers that Asset.
pub fn offers(c: &Connection, basket: &Basket, side: Side, id: i64) -> Result<bool, WebError> {
    let (venue, quote, members, count, excluded) = offered_params(basket, side);
    let sql = format!("SELECT EXISTS(SELECT 1 FROM assets WHERE id = ?6 AND id IN (SELECT {} FROM ({})))", side.column(), offered(side));
    Ok(c.query_row(&sql, params![venue, quote, members, count, excluded, id], |r| r.get(0))?)
}

/// Bots::Searchable#parse_exchanges: Binance.US shown as Binance where Binance trades, then `uniq.sort`.
pub fn parse_exchanges(exchanges: Vec<(String, String)>, binance_name: Option<&str>) -> Vec<(String, String)> {
    let mut parsed: Vec<(String, String)> = exchanges.into_iter().map(|(name_id, name)| match binance_name {
        Some(binance) if name_id == "binance_us" => ("binance".to_string(), binance.to_string()),
        _ => (name_id, name),
    }).collect();
    parsed.sort();
    parsed.dedup();
    parsed
}

/// Bots::Searchable#attach_exchanges: the available venues each page row is offered on.
pub fn attach_exchanges(c: &Connection, basket: &Basket, side: Side, page: &mut [Row]) -> Result<(), WebError> {
    if page.is_empty() { return Ok(()); }
    let (venue, quote, members, count, excluded) = offered_params(basket, side);
    let ids = serde_json::json!(page.iter().map(|row| row.id).collect::<Vec<_>>()).to_string();
    let column = side.column();
    let sql = format!("SELECT DISTINCT o.{column}, e.type, e.name FROM ({}) o JOIN exchanges e ON e.id = o.exchange_id \
                       WHERE e.available = 1 AND o.{column} IN (SELECT value FROM json_each(?6))", offered(side));
    let mut statement = c.prepare(&sql)?;
    let offered: Vec<(i64, String, Option<String>)> = statement.query_map(params![venue, quote, members, count, excluded, ids], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    let binance: Option<String> = c.query_row(
        "SELECT name FROM exchanges e WHERE e.available = 1 AND e.type = 'Exchanges::Binance' \
         AND EXISTS(SELECT 1 FROM tickers t WHERE t.exchange_id = e.id AND t.available = 1 AND t.trading_enabled = 1) ORDER BY id LIMIT 1",
        [], |r| r.get::<_, Option<String>>(0)).optional()?.flatten();
    for row in page.iter_mut() {
        let listed = offered.iter().filter(|(id, _, _)| *id == row.id).map(|(_, class, name)| {
            let exchange = super::Exchange { id: 0, name: name.clone().unwrap_or_default(), class: class.clone(), maker_fee: None };
            (exchange.name_id(), exchange.name)
        }).collect();
        row.exchanges = parse_exchanges(listed, binance.as_deref());
    }
    Ok(())
}

/// `all_assets[offset, ASSET_PAGE_SIZE] || []`, Ruby's slice with a negative start counted from the end.
pub fn page_of(rows: Vec<Row>, offset: i64) -> Vec<Row> {
    let size = rows.len() as i64;
    let start = if offset < 0 { offset + size } else { offset };
    if start < 0 || start > size { return vec![]; }
    rows.into_iter().skip(start as usize).take(PAGE_SIZE).collect()
}

/// A row as the result partial shows it.
pub struct Item {
    pub(crate) id: i64,
    pub(crate) symbol: String,
    pub(crate) name: String,
    pub(crate) ticker_class: &'static str,
    pub(crate) pill: String,
    pub(crate) logo: String,
    pub(crate) exchanges: Vec<(String, &'static str)>,
}

pub(crate) fn items(rows: Vec<Row>) -> Result<Vec<Item>, WebError> {
    rows.into_iter().map(|row| Ok(Item {
        id: row.id,
        ticker_class: colors::ticker_class(row.category.as_deref(), row.color.as_deref()),
        pill: colors::pill_color(row.color.as_deref()).ok_or_else(|| WebError::Config("an asset colour Ruby cannot read".into()))?,
        logo: crate::web::tracker::row::asset_logo(row.image_url, row.color, row.symbol.clone(), row.category)?,
        symbol: row.symbol,
        name: row.name.unwrap_or_default(),
        exchanges: row.exchanges.into_iter().map(|(name_id, name)| (name, exchange_svg(&name_id))).collect(),
    })).collect()
}

#[derive(Template)]
#[template(path = "bots/asset_searches/edit.html")]
struct Edit<'a> { v: &'a Ctx, csrf: &'a str, search_url: String, bot_url: String, asset_field: Option<&'a str>, field_name: String, items: Vec<Item>, next: Option<(String, String)> }

#[derive(Template)]
#[template(path = "bots/_asset_page.html")]
pub(crate) struct AssetPage { pub(crate) frame_id: String, pub(crate) field_name: String, pub(crate) items: Vec<Item>, pub(crate) next: Option<(String, String)> }

/// GET /bots/:id/asset_search/edit.
pub async fn edit(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(segment): Path<String>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().cloned() else { return Ok(auth::unauthenticated(&ctx)) };
    let (inner, view, owner) = (app.clone(), ctx.clone(), user.clone());
    let result = app.db(move |c| {
        let tx = c.unchecked_transaction()?;
        let (found, _) = page::find(&tx, &inner, &owner, &segment, false, For::Settings, view.locale)?;
        let Found::Bot(bot) = found else { return Ok(Err(found)) };
        if bot.kind != Kind::Basket { return Ok(Err(Found::NotPorted("the asset search of an index bot"))); }
        let params = &view.params;
        let asset_field = params.query("asset_field");
        let side = if asset_field == Some("quote_asset_id") { Side::Quote } else { Side::Base };
        let mut members: Vec<i64> = match bot.settings.get("allocations").and_then(serde_json::Value::as_object) {
            Some(weights) if !weights.is_empty() => weights.keys().map(|key| to_i(key)).collect(),
            _ => vec![],
        };
        if members.is_empty() {
            members = bot.settings.get("base_asset_ids").and_then(serde_json::Value::as_array).map_or_else(Vec::new, |ids| {
                ids.iter().map(|id| id.as_i64().unwrap_or_else(|| id.as_str().map_or(0, to_i))).collect()
            });
        }
        let basket = Basket { exchange_id: Some(bot.exchange.id), quote_asset_id: bot.setting("quote_asset_id").and_then(serde_json::Value::as_i64), base_asset_ids: members };
        let offset = params.query("offset").map_or(0, to_i);
        let all = filtered_rows(&tx, &basket, side, params.query("query"))?;
        let next_offset = offset.saturating_add(PAGE_SIZE as i64);
        let next = (all.len() as i64 > next_offset).then(|| {
            let query = params.fullpath.split_once('?').map_or("", |(_, query)| query);
            (format!("assets-page-{next_offset}"), format!("{}?{}", params.full_path, next_page_query(query, next_offset)))
        });
        let mut rows = page_of(all, offset);
        attach_exchanges(&tx, &basket, side, &mut rows)?;
        let field_name = format!("bots_dca_multi_asset[{}]", asset_field.unwrap_or(""));
        let items = items(rows)?;
        if offset > 0 {
            let body = AssetPage { frame_id: format!("assets-page-{offset}"), field_name, items, next }.render()?;
            tx.commit()?;
            return Ok(Ok((body, None)));
        }
        let csrf = view.csrf_token();
        let search_url = match asset_field {
            Some(field) => format!("{}?asset_field={}", view.path(&format!("/bots/{}/asset_search/edit", bot.id)), cgi_escape(field)),
            None => view.path(&format!("/bots/{}/asset_search/edit", bot.id)),
        };
        let body = Edit { v: &view, csrf: &csrf, search_url, bot_url: view.path(&format!("/bots/{}", bot.id)), asset_field, field_name, items, next }.render()?;
        let shell = Shell::load(&tx, &inner, &owner)?;
        tx.commit()?;
        Ok(Ok((body, Some((csrf, shell)))))
    }).await;
    match result {
        Ok(Ok((body, None))) => Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response()),
        Ok(Ok((body, Some((csrf, shell))))) => shell::application(&ctx, &csrf, &user, &shell, Page { status: StatusCode::OK, body, flash_now: vec![] }),
        Ok(Err(Found::Missing)) => Ok(page::not_found(&ctx)),
        Ok(Err(Found::NotPorted(reason))) => Ok(layout::refused(&ctx, reason)),
        Ok(Err(Found::Bot(_))) => Err(WebError::Config("unexpected asset search result".into())),
        Err(e) => layout::or_refused(&ctx, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binance_us_folds_into_binance_where_binance_trades() {
        let listed = vec![("binance_us".to_string(), "Binance.US".to_string()), ("binance".to_string(), "Binance".to_string()), ("alpaca".to_string(), "Alpaca".to_string())];
        assert_eq!(parse_exchanges(listed.clone(), Some("Binance")), vec![("alpaca".into(), "Alpaca".into()), ("binance".into(), "Binance".into())]);
        assert_eq!(parse_exchanges(listed, None).len(), 3);
    }

    #[test]
    fn ruby_slices_and_to_i() {
        let rows = |n: i64| (0..n).map(|id| Row { id, symbol: "X".into(), name: None, color: None, category: None, image_url: None, market_cap_rank: None, exchanges: vec![] }).collect::<Vec<_>>();
        assert_eq!(page_of(rows(25), 20).len(), 5);
        assert_eq!(page_of(rows(25), 25).len(), 0);
        assert_eq!(page_of(rows(25), -5).first().map(|row| row.id), Some(20));
        assert!(page_of(rows(25), -26).is_empty());
        assert_eq!((to_i("20"), to_i(" 2_0x"), to_i("-3"), to_i("abc"), to_i("_1")), (20, 20, -3, 0, 0));
    }
}
