//! The populated index's regions (TrackerController#index): the pending report, the venue filters, the six tiles, the
//! holdings card and the sync warnings. The chart and the record are not ported yet, so no GET serves a page with these
//! regions (`index` keeps the 501); `regions` is what that page will place, and what the grid compares with Rails.
use super::{fx::Prepared, Ctx, WebError};
use crate::figures::{dec::Dec, totals::Denomination};
use crate::tracker::figures::{page, Figures, Note};
use crate::web::i18n::{self, escape, Arg};
use askama::Template;
use rusqlite::{Connection, OptionalExtension};

fn invalid() -> WebError { super::row::invalid() }
fn dec(raw: &str) -> Result<Dec, WebError> { Dec::parse(raw).map_err(|_| invalid()) }
fn num<T>(r: Result<T, crate::figures::num::NumError>) -> Result<T, WebError> { r.map_err(|_| invalid()) }

/// What the page places, region by region, in Rails' order.
pub struct Regions { pub pending: String, pub filters: String, pub chart: String, pub tiles: String, pub holdings: String, pub record: String, pub warnings: String }

/// `rescue_from Tracker::Figures::AmbiguousAssetClass`: the page is a 422 with the message.
pub enum Built { Regions(Regions), Refused(String) }

struct Venue { id: i64, name: String, kind: String }
fn venues(c: &Connection, sql: &str, owner: i64) -> Result<Vec<Venue>, WebError> {
    let mut s = c.prepare(sql)?;
    let rows = s.query_map([owner], |r| Ok(Venue { id: r.get(0)?, name: r.get(1)?, kind: r.get(2)? }))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}
fn name_id(kind: &str) -> String { crate::tracker::rows::name_id(kind) }
fn stock_venue(kind: &str) -> bool { matches!(kind, "Exchanges::Alpaca" | "Exchanges::Ibkr") }

/// A route helper's path with its query, as `url_for` writes one (keys sorted, nil values left out).
fn with_query(ctx: &Ctx, path: &str, pairs: &[(&str, Option<&str>)]) -> String {
    let pairs: Vec<(String, String)> = pairs.iter().filter_map(|(k, v)| v.map(|v| (k.to_string(), v.to_string()))).collect();
    let query = crate::web::locale::switch_query(&pairs);
    ctx.path(&if query.is_empty() { path.to_string() } else { format!("{path}?{query}") })
}

// ---- check_pending_report -------------------------------------------------------------------------------------------

/// `Tax::GenerateReportJob.report_path`.
fn report_path(ctx: &Ctx, owner: i64, country: &str, year: i64, scope: &str) -> std::path::PathBuf {
    let scope = if matches!(scope, "crypto" | "broker") { scope } else { "crypto" };
    let country: String = country.chars().filter(char::is_ascii_alphabetic).collect::<String>().to_uppercase();
    ctx.app.config.tmp_dir.join("tax_reports").join(format!("{owner}_{country}_{year}_{scope}.csv"))
}

/// Ruby's `to_i` on a JSON value as the settings hold it.
fn to_i(v: &serde_json::Value) -> i64 {
    match v {
        serde_json::Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        serde_json::Value::String(s) => { let t = s.trim_start(); let end = t.char_indices().find(|(i, ch)| !(ch.is_ascii_digit() || (*i == 0 && (*ch == '-' || *ch == '+')))).map_or(t.len(), |(i, _)| i); t[..end].parse().unwrap_or(0) }
        _ => 0,
    }
}
fn ruby_s(v: &serde_json::Value) -> String { match v { serde_json::Value::String(s) => s.clone(), serde_json::Value::Null => String::new(), other => other.to_string() } }

pub(super) fn pending_report(ctx: &Ctx, owner: i64, settings: &serde_json::Value) -> Result<String, WebError> {
    let Some(pending) = settings.get("pending_report").filter(|v| v.is_object()) else { return Ok(String::new()) };
    let truthy = |v: &serde_json::Value| !matches!(v, serde_json::Value::Null | serde_json::Value::Bool(false));
    let (country, year) = (&pending["country"], &pending["year"]);
    if !truthy(country) || !truthy(year) { return Ok(String::new()); }
    let scope = match &pending["report_scope"] { serde_json::Value::String(s) if !crate::ruby::blank(s) => s.clone(), serde_json::Value::Null | serde_json::Value::Bool(false) => "crypto".into(), serde_json::Value::String(_) => "crypto".into(), other => other.to_string() };
    let path = report_path(ctx, owner, &ruby_s(country), to_i(year), &scope);
    let refusal = std::path::PathBuf::from(format!("{}.refused.json", path.display()));
    if refusal.exists() {
        // `JSON.parse` failing reads as no refusal.
        if let Ok(refusal) = serde_json::from_slice::<serde_json::Value>(&std::fs::read(&refusal).map_err(|e| WebError::Config(e.to_string()))?) {
            let key = match refusal["reason"].as_str() { Some("excess_return_of_capital") => "tracker.tax_report.refused_excess_return_of_capital", _ => "tracker.tax_report.refused_tokenized" };
            let symbols = refusal["symbols"].as_array().ok_or_else(invalid)?.iter().map(ruby_s).collect::<Vec<_>>().join(", ");
            let message = escape(&i18n::text(ctx.locale, key, &[("symbols", Arg::Text(&symbols))]));
            return Ok(Refused { message }.render()?);
        }
    }
    if !path.exists() { return Ok(String::new()); }
    let url = with_query(ctx, "/tracker/download_tax_report", &[("country", Some(&ruby_s(country))), ("report_scope", Some(&scope)), ("year", Some(&ruby_s(year)))]);
    Ok(format!("  <div data-controller=\"auto-download\" data-auto-download-url-value=\"{}\"></div>\n", escape(&url)))
}
#[derive(Template)]
#[template(path = "tracker/report_refused.html")]
struct Refused { message: String }

// ---- the figures ----------------------------------------------------------------------------------------------------

/// What the page states from `load_ledgers` and `load_portfolio`.
struct Portfolio {
    warm: Option<page::Warm>,
    /// The scoped ledger the warm figures were computed from (`@scoped_ledger`).
    ledger: Option<Box<crate::tracker::walk::Summary>>,
    /// The warm entry's publication, which the history-gap row is read against.
    published: Option<crate::tracker::cache::Fingerprint>,
    cold: page::Cold,
    has_keys: bool,
    never_synced: bool,
}

fn portfolio(c: &Connection, ctx: &Ctx, owner: i64, scope: Option<i64>, show_cash: bool) -> Result<Result<Portfolio, String>, WebError> {
    let figures_error = |e: crate::figures::FiguresError| match e { crate::figures::FiguresError::Sqlite(e) => WebError::from(e), _ => invalid() };
    let cold = crate::figures::budget::within(|| page::cold(c, owner, scope, show_cash)).map_err(figures_error)?;
    let now = ctx.app.now();
    let (mut ledger, mut published) = (None, None);
    let entry = crate::figures::budget::within(|| crate::tracker::cache::read_published(c, owner, now)).map_err(figures_error)?;
    let from = match &entry { crate::tracker::cache::Published::Warm { from, .. } => Some(from.clone()), _ => None };
    let warm = match entry.scoped(scope) {
        crate::tracker::cache::State::Warm(summary) => {
            let fx = crate::figures::budget::within(|| crate::tracker::fx::FxTable::load(c)).map_err(figures_error)?;
            match crate::figures::budget::within(|| page::warm(c, owner, scope, show_cash, &summary, &fx, now.date_naive())) {
                Ok(warm) => { ledger = Some(summary); published = from; Some(warm) }
                Err(crate::figures::FiguresError::NotComputed(message)) if message.contains("ambiguous asset class") => return Ok(Err(message)),
                Err(e) => return Err(figures_error(e)),
            }
        }
        // A ledger still warming, or one whose calculation failed: `Ledger.cached` is nil either way.
        crate::tracker::cache::State::Cold | crate::tracker::cache::State::Failed(_) => None,
    };
    let has_keys: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM api_keys WHERE user_id=?1 AND status=1 AND key_type!=1)", [owner], |r| r.get(0))?;
    let balances: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM account_balances WHERE user_id=?1 AND (?2 IS NULL OR exchange_id=?2) AND free+locked>0)", rusqlite::params![owner, scope], |r| r.get(0))?;
    let marked: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM api_keys WHERE user_id=?1 AND balances_synced_at IS NOT NULL)", [owner], |r| r.get(0))?;
    Ok(Ok(Portfolio { warm, ledger, published, cold, has_keys, never_synced: has_keys && !balances && !marked }))
}

fn signed(d: &Dec, den: &Denomination) -> Result<String, WebError> { Ok(format!("{}{}", if d.is_negative() { "" } else { "+" }, money(d, den)?)) }
fn tone(d: &Dec) -> &'static str { if d.is_negative() { "text-danger" } else { "text-success" } }
fn money(d: &Dec, den: &Denomination) -> Result<String, WebError> { super::row::formatted(&num(d * &den.rate)?, den) }
fn plain(d: &Dec, den: &Denomination) -> Result<String, WebError> { super::row::plain(&num(d * &den.rate)?, den) }
fn no_value(reason: &str) -> String { format!("<span class=\"no-value\" title=\"{}\">—</span>", escape(reason)) }

struct Tile { label: String, hint: Option<String>, value: Option<String>, class: &'static str }
/// bots/_metrics_item, six times, in the tiles' grid.
fn render_tiles(tiles: &[Tile]) -> String {
    let mut out = String::from("<div class=\"widget data-grid data-grid--six\">\n");
    for t in tiles {
        out.push_str(&format!("    <div class=\"data-grid__item\" data-controller=\"tooltip\">\n  <div class=\"label\">\n    {}\n", t.label));
        if let Some(hint) = &t.hint {
            out.push_str(&format!("      <span class=\"data-grid__item__hint\" aria-label=\"{hint}\"\n            data-controller=\"tooltip\" data-tooltip-fixed-value=\"true\"\n            data-action=\"mouseenter->tooltip#showTooltip mouseleave->tooltip#hideTooltip\">\n        {}\n        <div class=\"tooltip tooltip--hint\">{hint}</div>\n      </span>\n", include_str!("../../../templates/svg/_24x24_info.html")));
        }
        let value = t.value.clone().unwrap_or_else(|| "<div class=\"loader--small\" style=\"position: unset; margin: 0.5rem 0 0 auto;\"></div>".into());
        out.push_str(&format!("  </div>\n  <div class=\"data-grid__item__value {}\">\n      {value}\n  </div>\n</div>\n\n", t.class));
    }
    out.push_str("  </div>\n\n    ");
    out
}

/// The six tiles. `withheld` is why no money is stated (SPEC's never-a-number rule): balances that are stale, unpriced
/// or not produced by a current key withhold what they decide (money in, value, unrealised, total); with no display rate
/// every amount is withheld, since none can be written in it.
fn tiles(ctx: &Ctx, p: &Portfolio, den: Option<&Denomination>, withheld: Option<&str>) -> Result<String, WebError> {
    if ctx.user().ok_or_else(invalid)?.hide_balances { return Ok(String::new()); }
    let label = |key: &str| i18n::text(ctx.locale, key, &[]);
    let (value_label, invested_label) = (label("bot.details.stats.portfolio_value"), label("bot.details.stats.total_invested"));
    let total_hint = i18n::text(ctx.locale, "tracker.tiles.total_pnl_hint", &[("value", Arg::Text(&value_label)), ("invested", Arg::Text(&invested_label))]);
    let tile = |label: String, hint: Option<String>, value: Option<String>, class: &'static str| Tile { label: escape(&label), hint: hint.map(|h| escape(&h)), value, class };
    let reason = withheld;
    let tiles = match &p.warm {
        Some(warm) => {
            let f = &warm.figures;
            // A withheld cell states its reason; a stated one needs the display currency (withheld when there is none).
            let cell = |blank: Option<&str>, amount: &dyn Fn(&Denomination) -> Result<String, WebError>| -> Result<Option<String>, WebError> {
                match (blank, den) { (Some(r), _) => Ok(Some(no_value(r))), (None, Some(den)) => amount(den).map(Some), (None, None) => Ok(Some(no_value("Currency conversion unavailable"))) }
            };
            let toned = |blank: Option<&str>, d: &Dec| if blank.is_some() || den.is_none() { "" } else { tone(d) };
            let received = Some(&warm.received).filter(|r| r.is_positive());
            let hint = match (received, den, reason) { (Some(r), Some(den), None) => Some(i18n::text(ctx.locale, "tracker.tiles.received_hint", &[("received", Arg::Text(&plain(r, den)?))])), _ => None };
            vec![
                tile(invested_label, hint, cell(reason, &|den| money(&f.invested, den))?, ""),
                tile(value_label, None, cell(reason, &|den| money(&f.value, den))?, ""),
                tile(label("tracker.fees_paid"), None, cell(None, &|den| money(&f.fees, den))?, "data-grid__item__value--quiet"),
                tile(label("bot.dca_index.realised_pnl"), None, cell(None, &|den| signed(&f.realised, den))?, toned(None, &f.realised)),
                tile(label("tracker.tiles.unrealised_pnl"), None, cell(reason, &|den| signed(&f.unrealised, den))?, toned(reason, &f.unrealised)),
                tile(label("tracker.tiles.total_pnl"), Some(total_hint), cell(reason, &|den| signed(&f.total, den))?, toned(reason, &f.total)),
            ]
        }
        _ => {
            // The value is known from the balances alone; everything the history decides waits for it.
            let value = match (reason, &p.cold.value, den) { (Some(r), _, _) => no_value(r), (None, Some(v), Some(den)) => money(v, den)?, _ => no_value("Currency conversion unavailable") };
            vec![
                tile(invested_label, None, None, ""), tile(value_label, None, Some(value), ""), tile(label("tracker.fees_paid"), None, None, "data-grid__item__value--quiet"),
                tile(label("bot.dca_index.realised_pnl"), None, None, ""), tile(label("tracker.tiles.unrealised_pnl"), None, None, ""), tile(label("tracker.tiles.total_pnl"), Some(total_hint), None, ""),
            ]
        }
    };
    Ok(render_tiles(&tiles))
}

// ---- the holdings card ----------------------------------------------------------------------------------------------

struct AssetRow { symbol: String, name: String, color: Option<String>, image: Option<String>, category: Option<String> }
fn asset_row(c: &Connection, id: i64) -> Result<AssetRow, WebError> {
    let row = c.query_row("SELECT symbol,name,color,image_url,category FROM assets WHERE id=?1", [id], |r| Ok(AssetRow { symbol: r.get::<_, Option<String>>(0)?.unwrap_or_default(), name: r.get::<_, Option<String>>(1)?.unwrap_or_default(), color: r.get(2)?, image: r.get(3)?, category: r.get(4)? })).optional()?;
    row.ok_or_else(invalid)
}

/// `holding_type_label`.
fn type_label(symbol: &str, category: Option<&str>) -> &'static str {
    if crate::tracker::stable(symbol) { return "Stable"; }
    match category { Some("Cryptocurrency") => "Crypto", Some("Stock" | "Common Stock") => "Stock", Some("ETF") => "ETF", Some("Fund") => "Fund", Some("Fiat" | "Currency") => "Cash", _ => "" }
}

/// `tracker_percent`: signed, one decimal.
fn percent_text(d: &Dec) -> Result<String, WebError> { Ok(format!("{}{}%", if d.is_negative() { "" } else { "+" }, fixed(d, 1)?)) }
fn fixed(d: &Dec, digits: i64) -> Result<String, WebError> {
    // number_with_precision writes a figure that rounds to zero without its minus.
    let text = num(d.round(digits))?.to_s_f();
    let text = if text.trim_start_matches('-').chars().all(|c| c == '0' || c == '.') { text.trim_start_matches('-').to_string() } else { text };
    let (whole, fraction) = text.split_once('.').ok_or_else(invalid)?;
    Ok(format!("{whole}.{fraction:0<width$}", width = digits as usize))
}

/// `tracker_note`: the assumption in words, its money masked while balances are hidden.
pub(super) fn note_text(ctx: &Ctx, n: &Note, den: &Denomination, hidden: bool) -> Result<String, WebError> {
    let quantity = |units: &Option<Dec>| -> Result<String, WebError> {
        if hidden && crate::tracker::cash(&n.symbol) { return Ok("•••".into()); }
        units.as_ref().map(|u| super::row::amount_text(u, &n.symbol)).transpose().map(Option::unwrap_or_default)
    };
    let exchange = n.exchange.clone().unwrap_or_else(|| i18n::text(ctx.locale, "tracker.notes.your_exchanges", &[]));
    let amount = if hidden { "•••".to_string() } else { plain(&n.amount, den)? };
    let (history, held) = (quantity(&n.history)?, quantity(&n.held)?);
    Ok(i18n::text(ctx.locale, &format!("tracker.notes.{}", n.kind), &[("symbol", Arg::Text(&n.symbol)), ("exchange", Arg::Text(&exchange)), ("history", Arg::Text(&history)), ("held", Arg::Text(&held)), ("amount", Arg::Text(&amount))]))
}

pub(super) struct CardRow { asset: AssetRow, value: Dec, percent: Option<Dec>, note: Option<String> }
impl CardRow {
    /// A row of the warming card (`waiting_for_ledger`): its balances' value, no cost and no note.
    pub(super) fn cold(h: &page::Holding) -> CardRow {
        CardRow { asset: AssetRow { symbol: h.symbol.clone(), name: h.name.clone(), color: h.color.clone(), image: h.image.clone(), category: h.category.clone() }, value: h.value.clone(), percent: None, note: None }
    }
}
#[derive(Template)]
#[template(path = "tracker/holding.html")]
struct RowView<'a> { v: &'a Ctx, hidden: bool, symbol: String, name: String, logo: String, kind: String, color: String, share: String, percent: String, value: String, pnl: Option<(String, &'static str)>, note: Option<String> }
#[derive(Template)]
#[template(path = "tracker/holdings.html")]
struct Card<'a> { v: &'a Ctx, hidden: bool, count: usize, shares: String, arcs: Vec<super::holdings::Arc>, shown: Vec<String>, folded: Vec<String>, empty: Option<String>, centre: Option<(String, &'static str)>, unpriced: Option<String>, asides: Vec<String> }

pub(super) fn card(ctx: &Ctx, rows: Vec<CardRow>, centre: Option<Dec>, empty: Option<String>, asides: Vec<String>, den: &Denomination) -> Result<String, WebError> {
    let hidden = ctx.user().ok_or_else(invalid)?.hide_balances;
    let total = rows.iter().try_fold(Dec::zero(), |sum, r| &sum + &r.value).map_err(|_| invalid())?;
    let other = i18n::text(ctx.locale, "tracker.other", &[]);
    let mut shares: Vec<(String, Dec)> = vec![];
    let mut views = vec![];
    for r in &rows {
        let kind = type_label(&r.asset.symbol, r.asset.category.as_deref());
        let group = if kind.is_empty() { other.clone() } else { kind.into() };
        match shares.iter_mut().find(|(k, _)| *k == group) { Some((_, v)) => *v = num(&*v + &r.value)?, None => shares.push((group, r.value.clone())) }
        let share = if total.is_positive() { num(&num(r.value.div(&total))? * &dec("100")?)? } else { Dec::zero() };
        let color = crate::web::colors::ensure_contrast(r.asset.color.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or(crate::web::colors::NEUTRAL)).ok_or_else(invalid)?;
        views.push(RowView {
            v: ctx, hidden, symbol: escape(&r.asset.symbol), name: escape(&r.asset.name),
            logo: super::row::asset_logo(r.asset.image.clone(), r.asset.color.clone(), r.asset.symbol.clone(), r.asset.category.clone())?,
            kind: kind.into(), color, share: num(share.round(2))?.to_s_f(), percent: fixed(&share, 1)?, value: money(&r.value, den)?,
            pnl: r.percent.as_ref().map(|p| Ok::<_, WebError>((percent_text(p)?, tone(p)))).transpose()?, note: r.note.as_ref().map(|n| escape(n)),
        }.render()?);
    }
    let mut rounded = vec![];
    if total.is_positive() {
        for (label, value) in shares {
            let share = num(num(&num(value.div(&total))? * &dec("100")?)?.round(0))?;
            if !share.is_zero() { rounded.push((label, share)); }
        }
    }
    rounded.sort_by(|a, b| b.1.cmp(&a.1));
    let shares = rounded.into_iter().map(|(label, value)| format!("{label} {}%", value.to_s_f().trim_end_matches(".0"))).collect::<Vec<_>>().join(" · ");
    let slices: Vec<(Dec, Option<String>, String)> = rows.iter().map(|r| (r.value.clone(), r.asset.color.clone(), r.asset.symbol.clone())).collect();
    let folded = if views.len() > 6 { views.split_off(6) } else { vec![] };
    Ok(Card {
        v: ctx, hidden, count: rows.len(), shares: escape(&shares), arcs: super::holdings::ring_of(&slices, &other)?, shown: views, folded, empty,
        centre: centre.map(|c| Ok::<_, WebError>((percent_text(&c)?, tone(&c)))).transpose()?, unpriced: None, asides,
    }.render()?)
}

fn holdings(c: &Connection, ctx: &Ctx, p: &Portfolio, den: Option<&Denomination>, withheld: Option<&str>) -> Result<String, WebError> {
    let hidden = ctx.user().ok_or_else(invalid)?.hide_balances;
    if p.never_synced {
        return Ok(format!("<div class=\"widget tracker-holdings {}\">\n  <div class=\"tracker-holdings__head\">\n    <span class=\"label\">{} · 0</span>\n    <span class=\"label\"></span>\n  </div>\n\n    <p class=\"tracker-holdings__empty\">{}</p>\n\n</div>\n\n",
            if hidden { "tracker-holdings--anon" } else { "" }, ctx.t("tracker.holdings"), ctx.t("tracker.portfolio.never_synced")));
    }
    let reason = withheld;
    let (Some(den), None) = (den, reason) else {
        return Ok(format!("<div class=\"widget tracker-holdings\"><p class=\"tracker-holdings__empty\">{}</p></div>\n\n", escape(reason.unwrap_or("Currency conversion unavailable"))));
    };
    let empty = |all_cash: bool| if all_cash { Some(ctx.t("tracker.portfolio.only_cash")) } else if p.has_keys { Some(ctx.t("tracker.portfolio.no_balances")) } else { None };
    match &p.warm {
        Some(warm) => {
            let f: &Figures = &warm.figures;
            // Every note about a coin finds its row; a coin nobody holds any more gets no sentence.
            let notes_for = |symbol: &str| -> Result<Option<String>, WebError> {
                let texts = f.notes.iter().filter(|n| !n.symbol.is_empty() && n.symbol == symbol).map(|n| note_text(ctx, n, den, hidden)).collect::<Result<Vec<_>, _>>()?;
                Ok(if texts.is_empty() { None } else { Some(texts.join(" ")) })
            };
            let mut rows = vec![];
            for h in &f.holdings {
                let asset = asset_row(c, h.asset.ok_or_else(invalid)?)?;
                let note = notes_for(&asset.symbol)?;
                rows.push(CardRow { asset, value: h.value.clone(), percent: h.percent().map_err(|_| invalid())?, note });
            }
            let mut cost = Dec::zero();
            let mut covered = Dec::zero();
            for h in &f.holdings { if let Some(c) = &h.cost { cost = num(&cost + c)?; covered = num(&covered + &h.value)?; } }
            let centre = if cost.is_positive() { Some(num(&num(&num(covered.div(&cost))? - &Dec::one())? * &dec("100")?)?) } else { None };
            let asides = f.notes.iter().filter(|n| n.kind == "figures_disagree").map(|n| note_text(ctx, n, den, hidden).map(|t| escape(&t))).collect::<Result<Vec<_>, _>>()?;
            let empty = if rows.is_empty() { empty(warm.all_cash) } else { None };
            Ok(card(ctx, rows, centre, empty, asides, den)? + "\n\n")
        }
        None => {
            let rows = p.cold.holdings.iter().map(CardRow::cold).collect::<Vec<_>>();
            let empty = if rows.is_empty() { empty(p.cold.only_cash) } else { None };
            Ok(card(ctx, rows, None, empty, vec![], den)? + "\n\n")
        }
    }
}

// ---- the venue filters ----------------------------------------------------------------------------------------------

struct Chip { value: String, label: String, active: bool, href: String, broken: bool }
struct MenuItem { label: String, icon: &'static str, href: String }
#[derive(Template)]
#[template(path = "tracker/filters.html")]
struct Filters<'a> { v: &'a Ctx, csrf: String, chips: Vec<Chip>, all_label: String, connect_href: String, menu: Vec<MenuItem>, sync: String, syncable: bool, report: bool, show_cash: bool, export_path: String }

fn filters(c: &Connection, ctx: &Ctx, owner: i64, scope: Option<i64>, show_cash: bool, sync: String) -> Result<String, WebError> {
    let exchanges = venues(c, "SELECT id,name,type FROM exchanges WHERE id IN (SELECT exchange_id FROM api_keys WHERE user_id=?1 UNION SELECT exchange_id FROM account_transactions WHERE user_id=?1) ORDER BY name", owner)?;
    let connectable = venues(c, "SELECT id,name,type FROM exchanges WHERE available=1 AND type!='Exchanges::Bitmart' AND id NOT IN (SELECT exchange_id FROM api_keys WHERE user_id=?1 UNION SELECT exchange_id FROM account_transactions WHERE user_id=?1) ORDER BY name", owner)?;
    let (brokers, rest): (Vec<Venue>, Vec<Venue>) = connectable.into_iter().partition(|v| stock_venue(&v.kind));
    let valid: Vec<i64> = { let mut s = c.prepare("SELECT DISTINCT exchange_id FROM api_keys WHERE user_id=?1 AND status=1 AND key_type!=1")?; let ids = s.query_map([owner], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?; ids };
    let retired = |v: &Venue| crate::web::bot::retired_type(&v.kind);
    let broken = exchanges.iter().any(|v| !valid.contains(&v.id) && !retired(v));
    let mut chips = vec![];
    if exchanges.len() > 1 || broken {
        chips.push(Chip { value: "all".into(), label: escape(&i18n::text(ctx.locale, "tracker.filters.all", &[])), active: scope.is_none(), href: escape(&with_query(ctx, "/tracker", &[])), broken: false });
        for v in &exchanges {
            let ok = valid.contains(&v.id) || retired(v);
            chips.push(Chip {
                value: v.id.to_string(), label: escape(&v.name), active: ok && scope == Some(v.id),
                href: escape(&if ok { with_query(ctx, "/tracker", &[("exchange_id", Some(&v.id.to_string()))]) } else { with_query(ctx, "/tracker/add_api_key/new", &[("exchange_id", Some(&v.id.to_string()))]) }),
                broken: !ok,
            });
        }
    }
    let menu = brokers.iter().chain(rest.iter()).map(|v| MenuItem { label: escape(&v.name), icon: crate::web::bot::exchange_svg(&name_id(&v.kind)), href: escape(&with_query(ctx, "/tracker/add_api_key/new", &[("exchange_id", Some(&v.id.to_string()))])) }).collect();
    let pairs = ctx.params.query.iter().filter(|(key, _)| matches!(key.as_str(), "exchange_id" | "from" | "to")).cloned().collect::<Vec<_>>();
    let query = crate::web::locale::switch_query(&pairs);
    let export_path = escape(&ctx.path(&format!("/tracker/export{}", if query.is_empty() { String::new() } else { format!("?{query}") })));
    Ok(Filters {
        v: ctx, csrf: ctx.csrf_token(), all_label: escape(&i18n::text(ctx.locale, "tracker.filters.all", &[])), chips, connect_href: ctx.path("/tracker/pick_exchange/new"),
        menu, sync, syncable: !valid.is_empty(), report: !exchanges.is_empty(), show_cash, export_path,
    }.render()?)
}

// ---- the page -------------------------------------------------------------------------------------------------------

pub fn regions(c: &Connection, ctx: &Ctx, owner: i64, fx: &Prepared) -> Result<Built, WebError> {
    // One snapshot for every input: the balances `cold` approves, the cached ledger and the balances `warm` computes
    // from are the same rows, whatever a sync commits meanwhile.
    if c.is_autocommit() { let tx = c.unchecked_transaction()?; let out = regions(&tx, ctx, owner, fx)?; tx.commit()?; return Ok(out); }
    let scope = ctx.params.query("exchange_id").filter(|s| !s.trim().is_empty()).map(|s| crate::web::bot::id_from_path(s).ok_or_else(invalid)).transpose()?;
    let raw: Option<String> = c.query_row("SELECT tracker_settings FROM users WHERE id=?1", [owner], |r| r.get(0))?;
    let settings: serde_json::Value = match raw.as_deref() { Some(text) => serde_json::from_str(text).map_err(|_| invalid())?, None => serde_json::Value::Null };
    let show_cash = crate::web::bots::show_cash(raw.as_deref());
    let p = match portfolio(c, ctx, owner, scope, show_cash)? { Ok(p) => p, Err(message) => return Ok(Built::Refused(message)) };
    let den = fx.denomination(c, owner, ctx.app.now().timestamp())?;
    let withheld = if den.is_none() { Some("Currency conversion unavailable") } else { p.cold.reason };
    // An account whose first balance sync has not completed keeps its money withheld, a warm ledger or not: the card
    // says it was never synced, and no tile states a value the balances cannot vouch for.
    let sync = super::holdings::synced_label(ctx, p.cold.synced, p.cold.stale);
    // `Denomination.for` falls back to dollars when no rate is had: the chart and the positions are drawn in them.
    let usd = Denomination { currency: "USD".into(), rate: Dec::one() };
    let shown = den.as_ref().unwrap_or(&usd);
    // The history-gap line: read against the entry this page read; any doubt is no line and the page as it was.
    let gaps = crate::tracker::gaps::read(c, owner, scope, p.published.as_ref(), None).unwrap_or_else(|e| {
        crate::engine::log(&format!("[tracker] history gap for user {owner} not read: {}", crate::tracker::jobs::message(e))); vec![]
    });
    Ok(Built::Regions(Regions {
        pending: pending_report(ctx, owner, &settings)?,
        filters: filters(c, ctx, owner, scope, show_cash, sync)?,
        tiles: tiles(ctx, &p, den.as_ref(), withheld)?,
        holdings: holdings(c, ctx, &p, den.as_ref(), withheld)?,
        chart: super::record::chart(c, ctx, owner, scope, show_cash, shown)?,
        record: super::record::record(c, ctx, owner, scope, p.warm.as_ref().zip(p.ledger.as_deref()), den.as_ref(), shown, &gaps)?,
        warnings: crate::web::settings::keys::warnings(c, ctx, owner)?,
    }))
}

#[derive(Template)]
#[template(path = "tracker/populated.html")]
struct Page<'a> { v: &'a Ctx, preferences: String, tax_report: String, sync: String, body: String }

/// TrackerController#index for a populated account: every region, as Rails' index places them.
pub fn page(c: &Connection, ctx: &Ctx, owner: i64, fx: &Prepared) -> Result<axum::response::Response, WebError> {
    use axum::response::IntoResponse;
    let Some(user) = ctx.user() else { return Ok(crate::web::auth::unauthenticated(ctx)) };
    let r = match regions(c, ctx, owner, fx)? {
        Built::Regions(r) => r,
        Built::Refused(message) => return Ok((axum::http::StatusCode::UNPROCESSABLE_ENTITY, [(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")], message).into_response()),
    };
    let tiles = if r.tiles.is_empty() { "\n    ".to_string() } else { format!("  {}", r.tiles) };
    let body = format!("{}  <div class=\"tracker\">\n    {}\n    {}\n    {tiles}{}    {}\n  </div>\n", r.pending, r.filters, r.chart, r.holdings, r.record);
    let body = Page { v: ctx, preferences: format!("user_{owner}:preferences"), tax_report: format!("user_{owner}:tax_report"), sync: format!("user_{owner}:sync"), body }.render()?;
    let page = super::layout::Page { status: axum::http::StatusCode::OK, body, flash_now: vec![] };
    if ctx.turbo_frame.is_some() { return super::layout::frame(&ctx.csrf_token(), &page); }
    let shell = crate::web::shell::Shell::load(c, &ctx.app, user)?;
    let extra = format!("  <div id=\"sync-warnings\" data-turbo-permanent>\n    {}\n  </div>\n", r.warnings);
    crate::web::shell::application_with_flash_extra(ctx, &ctx.csrf_token(), user, &shell, page, &extra)
}

#[cfg(test)]
mod tests {
    /// number_with_precision: a percentage that rounds to zero is written without its minus (Rails' 0.0%).
    #[test]
    fn a_percentage_rounding_to_zero_has_no_minus() {
        let p = |v: &str| super::percent_text(&crate::figures::dec::Dec::parse(v).unwrap()).unwrap();
        assert_eq!(p("-0.03"), "0.0%");
        assert_eq!(p("-0.3"), "-0.3%");
        assert_eq!(p("0.04"), "+0.0%");
    }
}
