//! The populated index's chart (tracker/_chart, from the swept history) and record (tracker/_record: the transactions
//! table and the positions table), as Rails renders them.
use super::{Ctx, WebError};
use crate::figures::{at::At, dec::Dec, json::J, num::oj_float, totals::Denomination};
use crate::tracker::figures::{page, Figures};
use crate::tracker::walk::Summary;
use crate::web::bots::{Segmented, SegmentedOption};
use crate::web::figure::NO_VALUE;
use crate::web::i18n::{self, escape};
use askama::Template;
use chrono::NaiveDate;
use rusqlite::Connection;

fn invalid() -> WebError { super::row::invalid() }
fn num<T>(r: Result<T, crate::figures::num::NumError>) -> Result<T, WebError> { r.map_err(|_| invalid()) }

/// shared/_segmented as a radio group, as the page's ERB call sites place it (a leading newline, no trailing one).
fn segmented(label: String, key: Option<&str>, fluid: bool, options: Vec<SegmentedOption>) -> Result<String, WebError> {
    let html = Segmented { fluid, label, key, options, links: false }.render()?;
    Ok(format!("\n{}", html.lines().filter(|line| !line.is_empty()).collect::<Vec<_>>().join("\n").replace("->", "-&gt;").replace("</svg>\n", "</svg>\n\n")))
}
fn option(value: &'static str, label: String, active: bool) -> SegmentedOption { SegmentedOption { value, label, active, href: None } }

// ---- the chart ------------------------------------------------------------------------------------------------------

struct Day { date: NaiveDate, value: Dec, invested: Dec, held_value: Option<Dec>, held_cost: Option<Dec> }

/// `PortfolioSnapshot.series`: the whole portfolio's days, or the switched venue's, by date.
fn series(c: &Connection, owner: i64, scope: Option<i64>) -> Result<Vec<Day>, WebError> {
    let sql = if scope.is_some() { "SELECT date,value_usd,invested_usd,held_value_usd,held_cost_usd FROM portfolio_venue_snapshots WHERE user_id=?1 AND exchange_id=?2 ORDER BY date" }
              else { "SELECT date,value_usd,invested_usd,held_value_usd,held_cost_usd FROM portfolio_snapshots WHERE user_id=?1 AND ?2 IS NULL ORDER BY date" };
    let mut s = c.prepare(sql)?;
    let mut q = s.query(rusqlite::params![owner, scope])?;
    let mut out = vec![];
    let column = |v: rusqlite::types::ValueRef<'_>| crate::tracker::figures::scaled(v, 8).map_err(|_| invalid());
    while let Some(r) = q.next()? {
        crate::figures::budget::charge(1, 0).map_err(|_| invalid())?;
        let date: String = r.get(0)?;
        out.push(Day { date: NaiveDate::parse_from_str(&date, "%Y-%m-%d").map_err(|_| invalid())?, value: column(r.get_ref(1)?)?.unwrap_or_else(Dec::zero),
                       invested: column(r.get_ref(2)?)?.unwrap_or_else(Dec::zero), held_value: column(r.get_ref(3)?)?, held_cost: column(r.get_ref(4)?)? });
    }
    Ok(out)
}

/// A Float as Ruby's `Float#to_d` reads it: its shortest decimal.
fn float_dec(f: f64) -> Result<Dec, WebError> { num(Dec::parse(&oj_float(f))) }
fn to_f(d: &Dec) -> Result<f64, WebError> { d.to_s_f().parse().map_err(|_| invalid()) }

/// tracker/_chart: nothing for a portfolio with no history, a spinner while one is swept, else the bot chart's widget.
pub fn chart(c: &Connection, ctx: &Ctx, owner: i64, scope: Option<i64>, show_cash: bool, den: &Denomination) -> Result<String, WebError> {
    let hidden = ctx.user().ok_or_else(invalid)?.hide_balances;
    let mut history = series(c, owner, scope)?;
    let first: Option<String> = c.query_row("SELECT MIN(transacted_at) FROM account_transactions WHERE user_id=?1 AND (?2 IS NULL OR exchange_id=?2)", rusqlite::params![owner, scope], |r| r.get(0))?;
    let first = first.map(|t| crate::codec::parse_time(&t).map(|t| t.date_naive()).map_err(|_| invalid())).transpose()?;
    let today = ctx.app.now().date_naive();
    let yesterday = today.pred_opt().ok_or_else(invalid)?;
    // `load_history`: with the cash hidden, a day swept before the positions' reading existed is not drawn, and is
    // waited for while a sweep can still reach it.
    let mut unswept = false;
    if !show_cash {
        let (gone, kept): (Vec<Day>, Vec<Day>) = history.into_iter().partition(|d| d.held_cost.is_none());
        unswept = gone.iter().any(|d| first.is_some_and(|f| d.date >= f && d.date <= yesterday));
        history = kept;
    }
    let gap = first.is_some_and(|f| f < today && history.first().is_none_or(|d| d.date > f));
    let loading = history.is_empty() && (unswept || gap);
    if history.is_empty() && !loading { return Ok(String::new()); }
    let percent_class = if hidden { "widget--chart__pnl" } else { "widget--chart__percent" };
    if history.is_empty() {
        let pnl = if hidden { "" } else { "<div class=\"widget--chart__pnl\">&nbsp;</div>" };
        return Ok(format!("<div class=\"widget widget--chart\">\n    <div class=\"widget--chart__head\">\n      <div class=\"widget--chart__summary\">\n        <div class=\"widget--chart__date\">&nbsp;</div>\n        {pnl}\n        <div class=\"{percent_class}\">&nbsp;</div>\n      </div>\n    </div>\n    <div class=\"widget--chart__plot\">\n      <div class=\"loader\" style=\"position: relative; left: calc(50% - 2rem); top: calc(50% - 6rem);\"></div>\n    </div>\n</div>\n"));
    }
    // `chart_history_series`: the whole portfolio, or the positions inside it; normalized while balances are hidden.
    let values: Vec<Dec> = history.iter().map(|d| if show_cash { d.value.clone() } else { d.held_value.clone().unwrap_or_else(Dec::zero) }).collect();
    let invested: Vec<Dec> = history.iter().map(|d| if show_cash { d.invested.clone() } else { d.held_cost.clone().unwrap_or_else(Dec::zero) }).collect();
    let floats = |serie: &[Dec], f: &dyn Fn(&Dec) -> Result<Dec, WebError>| serie.iter().map(|d| to_f(&num(f(d)?.round(2))?)).collect::<Result<Vec<f64>, WebError>>();
    let (value_series, invested_series) = if hidden {
        let divisor = [invested.last(), values.last()].into_iter().flatten().find(|d| d.is_positive()).cloned().unwrap_or_else(Dec::one);
        let hundred = Dec::from_i64(100);
        let scaled = |d: &Dec| num(&num(d.div(&divisor))? * &hundred);
        (floats(&values, &scaled)?, floats(&invested, &scaled)?)
    } else {
        let converted = |d: &Dec| num(d * &den.rate);
        (floats(&values, &converted)?, floats(&invested, &converted)?)
    };
    let pnl = value_series.iter().zip(&invested_series).map(|(v, i)| to_f(&num(&float_dec(*v)? - &float_dec(*i)?)?)).collect::<Result<Vec<_>, _>>()?;
    let floats_json = |s: &[f64]| J::Arr(s.iter().map(|f| J::Float(*f)).collect());
    let labels = J::Arr(history.iter().map(|d| J::Str(format!("{}T00:00:00", d.date))).collect()).write();
    let unit = match den.currency.as_str() { "USD" => "$", "EUR" => "€", "GBP" => "£", "CHF" => "Fr.", "PLN" => "zł", other => other };
    let suffixed = matches!(den.currency.as_str(), "CHF" | "PLN");
    let attributes = format!(
        "data-controller=\"bot--chart\" data-action=\"mouseover@document-&gt;bot--chart#focus mouseleave@document-&gt;bot--chart#blur click@document-&gt;bot--chart#select\" data-bot--chart-bot-value=\"0\" data-bot--chart-quote-value=\"{}\" data-bot--chart-unit-value=\"{}\" data-bot--chart-suffixed-value=\"{suffixed}\" data-bot--chart-decimals-value=\"2\" data-bot--chart-labels-value=\"{}\" data-bot--chart-series-value=\"{}\" data-bot--chart-pnl-value=\"{}\" data-bot--chart-assets-value=\"{{}}\" data-bot--chart-pnl-only-value=\"{hidden}\"",
        escape(&den.currency), escape(unit), escape(&labels), escape(&J::Arr(vec![floats_json(&value_series), floats_json(&invested_series)]).write()), escape(&floats_json(&pnl).write()));
    let mut modes = String::new();
    if !hidden {
        if invested_series.iter().any(|f| *f > 0.0) {
            let label = |key: &str| i18n::text(ctx.locale, &format!("bot.details.stats.chart.{key}"), &[]);
            modes.push_str(&format!("              {}\n", segmented(label("modes"), Some("chart-mode"), true, vec![option("pnl", label("pnl"), true), option("value", label("value"), false)])?));
        }
        // `chart_range_options`: a window shorter than the history is a choice.
        let span = (history[history.len() - 1].date - history[0].date).num_days();
        let windows: Vec<SegmentedOption> = [("30", "tracker.range.30d", 30), ("365", "tracker.range.1y", 365)].into_iter().filter(|(_, _, days)| span > *days)
            .map(|(value, key, _)| option(value, i18n::text(ctx.locale, key, &[]), false)).collect();
        if !windows.is_empty() {
            let mut options = windows;
            options.push(option("all", i18n::text(ctx.locale, "tracker.range.all", &[]), true));
            modes.push_str(&format!("              <div class=\"filters\" data-action=\"segmented:change->bot--chart#range\">\n                {}\n              </div>\n", segmented(i18n::text(ctx.locale, "tracker.range.all", &[]), None, false, options)?));
        }
    }
    let money = if hidden { "" } else { "            <div class=\"widget--chart__pnl\" data-bot--chart-target=\"pnl\">&nbsp;</div>\n" };
    Ok(format!("<div class=\"widget widget--chart\">\n    <div {attributes}>\n      <div class=\"widget--chart__head\">\n        <div class=\"widget--chart__modes\" data-action=\"segmented:change->bot--chart#mode\">\n{modes}        </div>\n        <div class=\"widget--chart__summary\" data-bot--chart-target=\"summary\">\n          <div class=\"widget--chart__date\" data-bot--chart-target=\"date\">&nbsp;</div>\n{money}          <div class=\"{percent_class}\"\n               data-bot--chart-target=\"percent\">&nbsp;</div>\n        </div>\n      </div>\n      <div class=\"widget--chart__plot\">\n        <canvas data-bot--chart-target=\"analyzerChart\" width=\"10\" height=\"2\"></canvas>\n      </div>\n      <div class=\"widget--chart__axis\" data-bot--chart-target=\"axis\"></div>\n</div></div>\n"))
}

// ---- the record -----------------------------------------------------------------------------------------------------

/// TrackerController::ROW_LIMIT: the table shows a window; `?all=1` asks for the lot.
const ROW_LIMIT: i64 = 200;

/// One row of the positions table (`tracker_position_rows`).
struct Position { status: &'static str, symbol: String, asset: Option<i64>, opened: Option<At>, closed: Option<At>, invested: Option<Dec>, avg_buy: Option<Dec>, price: Option<Dec>, percent: Option<Dec>, cash: Option<Dec> }

/// `Ledger.symbol_index`: the user's own balance row of the symbol, else the catalogue's coin, then its currency.
fn symbol_asset(c: &Connection, owner: i64, symbol: &str) -> Result<Option<i64>, WebError> {
    use rusqlite::OptionalExtension;
    let held: Option<i64> = c.query_row("SELECT b.asset_id FROM account_balances b JOIN assets a ON a.id=b.asset_id WHERE b.user_id=?1 AND a.symbol=?2 ORDER BY b.id LIMIT 1", rusqlite::params![owner, symbol], |r| r.get(0)).optional()?;
    if held.is_some() { return Ok(held); }
    Ok(c.query_row("SELECT id FROM assets WHERE symbol=?1 AND category IN ('Cryptocurrency','Fiat','Currency') ORDER BY category='Cryptocurrency' DESC,id LIMIT 1", [symbol], |r| r.get(0)).optional()?)
}

/// `Ledger.trip_assets`: the asset a trip's own rows recorded between its opening and its close; none recorded, the
/// symbol's (`asset_index`); two, neither.
fn trip_asset(c: &Connection, owner: i64, scope: Option<i64>, symbol: &str, opened: Option<At>, closed: At) -> Result<Option<i64>, WebError> {
    let mut s = c.prepare("SELECT base_asset_id,transacted_at FROM account_transactions WHERE user_id=?1 AND (?2 IS NULL OR exchange_id=?2) AND base_currency=?3 AND base_asset_id IS NOT NULL")?;
    let rows = s.query_map(rusqlite::params![owner, scope, symbol], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    let from = opened.unwrap_or(closed);
    let mut own: Vec<i64> = vec![];
    let mut all: Vec<i64> = vec![];
    for (id, at) in &rows {
        let at = At::from_utc(crate::codec::parse_time(at).map_err(|_| invalid())?).ok_or_else(invalid)?;
        if !all.contains(id) { all.push(*id); }
        if at >= from && at <= closed && !own.contains(id) { own.push(*id); }
    }
    match own.len() {
        // `asset_index`: a symbol its rows recorded once is that asset; recorded as two, none; recorded never, the symbol's.
        0 => match all.len() { 0 => symbol_asset(c, owner, symbol), 1 => Ok(Some(all[0])), _ => Ok(None) },
        1 => Ok(Some(own[0])),
        _ => Ok(None),
    }
}

fn positions(c: &Connection, owner: i64, scope: Option<i64>, figures: &Figures, ledger: &Summary) -> Result<Vec<Position>, WebError> {
    let mut rows = vec![];
    for h in &figures.holdings {
        let symbol = h.symbol.clone();
        let per_unit = |d: &Dec| -> Result<Option<Dec>, WebError> { if h.quantity.is_positive() { Ok(Some(num(d.div(&h.quantity))?)) } else { Ok(None) } };
        let price = per_unit(&h.value)?;
        rows.push(match &h.cost {
            Some(cost) => Position { status: "open", symbol: symbol.clone(), asset: h.asset, opened: ledger.positions.iter().find(|p| p.symbol == symbol).and_then(|p| p.opened_at),
                                     closed: None, invested: Some(cost.clone()), avg_buy: per_unit(cost)?, price, percent: h.percent().map_err(|_| invalid())?, cash: h.unrealised.clone() },
            None => Position { status: "cash", symbol, asset: h.asset, opened: None, closed: None, invested: None, avg_buy: None, price, percent: None, cash: None },
        });
    }
    for t in &ledger.round_trips {
        let complete = !t.incomplete && t.invested.is_positive();
        let per_unit = |d: &Dec| -> Result<Option<Dec>, WebError> { if t.quantity.is_positive() { Ok(Some(num(d.div(&t.quantity))?)) } else { Ok(None) } };
        rows.push(Position {
            status: if t.incomplete { "closed" } else if t.realised.is_negative() { "loss" } else { "win" },
            symbol: t.symbol.clone(), asset: trip_asset(c, owner, scope, &t.symbol, t.opened_at, t.closed_at)?, opened: t.opened_at, closed: Some(t.closed_at),
            invested: Some(t.invested.clone()), avg_buy: per_unit(&t.invested)?, price: per_unit(&t.proceeds)?,
            percent: if complete { Some(num(&num(t.realised.div(&t.invested))? * &Dec::from_i64(100))?) } else { None },
            cash: if complete { Some(t.realised.clone()) } else { None },
        });
    }
    // `sort_by { opened || Time.at(0) }.reverse`.
    rows.sort_by_key(|r| r.opened.map_or(0, |a| a.0));
    rows.reverse();
    Ok(rows)
}

/// `tracker_holding_period`.
fn holding_period(from: At, to: chrono::DateTime<chrono::Utc>) -> String {
    let days = ((to.timestamp_nanos_opt().unwrap_or(i64::MAX) - from.0) as f64 / 86_400e9) as i64;
    if days < 60 { return format!("{days}d"); }
    if days < 365 { return format!("{}mo", (days as f64 / 30.0).round() as i64); }
    let years = format!("{:.1}", days as f64 / 365.0);
    format!("{}y", years.trim_end_matches(".0"))
}

/// `tracker_percent`.
fn percent_text(d: &Dec) -> Result<String, WebError> {
    // number_with_precision writes a figure that rounds to zero without its minus.
    let text = num(d.round(1))?.to_s_f();
    let text = if text.trim_start_matches('-').chars().all(|c| c == '0' || c == '.') { text.trim_start_matches('-').to_string() } else { text };
    let (whole, fraction) = text.split_once('.').ok_or_else(invalid)?;
    Ok(format!("{}{whole}.{fraction:0<1}%", if d.is_negative() { "" } else { "+" }))
}

fn type_label(symbol: &str, category: Option<&str>) -> &'static str {
    if crate::tracker::stable(symbol) { return "Stable"; }
    match category { Some("Cryptocurrency") => "Crypto", Some("Stock" | "Common Stock") => "Stock", Some("ETF") => "ETF", Some("Fund") => "Fund", Some("Fiat" | "Currency") => "Cash", _ => "" }
}

/// A positions row. Its amounts are in `den`, which is dollars when no display rate is held, as Rails' `Denomination.for`
/// falls back (the chart does the same; the transactions' Value cells state the rate is unavailable instead).
fn position_row(c: &Connection, ctx: &Ctx, r: &Position, den: &Denomination, hidden: bool) -> Result<String, WebError> {
    let zone = &ctx.user().ok_or_else(invalid)?.time_zone;
    let money = |d: &Option<Dec>| -> Result<String, WebError> { match d { Some(d) => super::row::formatted(&num(d * &den.rate)?, den), None => Ok(String::new()) } };
    let (logo, kind) = match r.asset {
        Some(id) => {
            let (image, color, symbol, category): (Option<String>, Option<String>, Option<String>, Option<String>) = c.query_row("SELECT image_url,color,symbol,category FROM assets WHERE id=?1", [id], |q| Ok((q.get(0)?, q.get(1)?, q.get(2)?, q.get(3)?)))?;
            let kind = type_label(symbol.as_deref().unwrap_or(""), category.as_deref());
            (super::row::asset_logo(image, color, symbol.unwrap_or_default(), category)?, if kind.is_empty() { String::new() } else { format!("<span class=\"pill pill--quiet\">{kind}</span>") })
        }
        None => (String::new(), String::new()),
    };
    let tone = match r.status { "open" => "info", "win" => "up", "loss" => "down", _ => "quiet" };
    let opened = r.opened.map(|a| crate::web::format::table_date(a.utc(), zone)).unwrap_or_default();
    let closed = r.closed.map(|a| format!("<small>→ {}</small>", crate::web::format::table_date(a.utc(), zone))).unwrap_or_default();
    let cells = if hidden { String::new() } else {
        let cash = match &r.cash { Some(d) => format!("{}{}", if d.is_negative() { "" } else { "+" }, money(&Some(d.clone()))?), None => String::new() };
        format!("    <td>{}</td>\n    <td>{}</td>\n    <td>{}</td>\n    <td class=\"tracker-row__cash\">{cash}</td>\n", money(&r.invested)?, money(&r.avg_buy)?, money(&r.price)?)
    };
    let pnl = match &r.percent {
        Some(p) => format!("    <td class=\"{}\">{}</td>\n", if p.is_negative() { "text-danger" } else { "text-success" }, percent_text(p)?),
        None => format!("    <td title=\"{}\">{NO_VALUE}</td>\n", ctx.t("tracker.basis_incomplete")),
    };
    let hold = r.opened.map(|a| holding_period(a, r.closed.map_or_else(|| ctx.app.now(), |c| c.utc()))).unwrap_or_default();
    Ok(format!("<tr class=\"tracker-row\" data-order-filter-target=\"row\" data-order-type=\"all {}\">\n  <td class=\"text-left tracker-row__when table__when\">\n    {opened}\n    {closed}\n  </td>\n  <td class=\"text-left\"><span class=\"tracker-row__asset\">{logo}{}</span></td>\n  <td class=\"text-left\">{kind}</td>\n  <td class=\"text-left\"><span class=\"pill pill--{tone}\">{}</span></td>\n{cells}{pnl}  <td>{hold}</td>\n</tr>\n",
        r.status, escape(&r.symbol), i18n::t(ctx.locale, &format!("tracker.record.{}", r.status), &[])))
}

/// `offered`: a filter with fewer than two real choices is no filter.
fn offered(options: Vec<SegmentedOption>) -> Vec<SegmentedOption> { if options.iter().filter(|o| o.value != "all").count() > 1 { options } else { vec![] } }

/// tracker/_record. `warm` is the scoped ledger and its figures (none while it warms: the positions pane spins).
pub fn record(c: &Connection, ctx: &Ctx, owner: i64, scope: Option<i64>, warm: Option<(&page::Warm, &Summary)>, den: Option<&Denomination>, shown: &Denomination) -> Result<String, WebError> {
    let hidden = ctx.user().ok_or_else(invalid)?.hide_balances;
    let all = ctx.params.query("all").is_some_and(|v| !v.trim().is_empty());
    let sql = format!("SELECT id FROM account_transactions WHERE user_id=?1 AND (?2 IS NULL OR exchange_id=?2) ORDER BY transacted_at DESC{}", if all { String::new() } else { format!(" LIMIT {ROW_LIMIT}") });
    let ids: Vec<i64> = { let mut s = c.prepare(&sql)?; let ids = s.query_map(rusqlite::params![owner, scope], |r| r.get(0))?.collect::<Result<Vec<_>, _>>()?; ids };
    let mut rows = vec![];
    for id in &ids { crate::figures::budget::charge(1, 0).map_err(|_| invalid())?; rows.push(super::row::Row::load(c, owner, *id)?.ok_or_else(invalid)?); }
    let record = |key: &str| i18n::text(ctx.locale, &format!("tracker.record.{key}"), &[]);
    // `tracker_type_filters`: the types the table holds, in the ledger's order, and Transfer for a linked pair.
    let mut types = vec![option("all", record("all"), true)];
    for (i, kind) in super::row::TYPES.iter().enumerate() {
        if rows.iter().any(|r| r.kind == i as i64) { types.push(option(kind, i18n::text(ctx.locale, &format!("tracker.types.{kind}"), &[]), false)); }
    }
    if rows.iter().any(super::row::Row::linked) { types.push(option("transfer", record("transfer"), false)); }
    let types = offered(types);
    let position_rows = match warm { Some((w, ledger)) => positions(c, owner, scope, &w.figures, ledger)?, None => vec![] };
    let mut statuses = vec![option("all", record("all"), true)];
    for status in ["open", "win", "loss", "closed", "cash"] { if position_rows.iter().any(|r| r.status == status) { statuses.push(option(status, record(status), false)); } }
    let statuses = offered(statuses);
    let column = |key: &str| ctx.t(&format!("tracker.columns.{key}"));
    let switch = segmented(i18n::text(ctx.locale, "tracker.positions", &[]), Some("tracker-record"), true,
                           vec![option("pos", i18n::text(ctx.locale, "tracker.positions", &[]), true), option("tx", i18n::text(ctx.locale, "tracker.transactions", &[]), false)])?;
    let filter = |label: String, options: Vec<SegmentedOption>| -> Result<String, WebError> { if options.is_empty() { Ok(String::new()) } else { Ok(format!("        {}\n", segmented(label, None, true, options)?)) } };
    let tx_table = if rows.is_empty() { format!("        <p class=\"tracker-empty\">{}</p>\n", ctx.t("tracker.no_results")) } else {
        let mut body = String::new();
        for row in &rows { body.push_str(&format!("              {}\n", match den { Some(den) => super::row::render(c, ctx, row, den)?, None => super::row::unavailable(row.id) })); }
        let amount = if hidden { String::new() } else { format!("<th>{}</th>", column("amount")) };
        let money = if hidden { String::new() } else { format!("                <th>{} <small>{}</small></th>\n                <th>{}</th>\n", column("quote_amount"), escape(den.map_or("$", |d| match d.currency.as_str() { "USD" => "$", "EUR" => "€", "GBP" => "£", "CHF" => "Fr.", "PLN" => "zł", other => other })), column("fee")) };
        format!("        <table class=\"table\">\n          <thead>\n            <tr>\n              <th class=\"text-left\">{}</th>\n              <th class=\"tracker-exchange-icon\"></th>\n              <th class=\"text-left\">{}</th>\n              <th class=\"text-left\">{}</th>\n              {amount}\n              <th>{}</th>\n{money}              <th>{}</th>\n            </tr>\n          </thead>\n          <tbody>\n{body}          </tbody>\n        </table>\n",
            column("date"), column("type"), column("base"), column("price"), column("bot"))
    };
    let positions_table = if warm.is_none() { "        <div class=\"loader--small\" style=\"position: unset; margin: 2rem auto;\"></div>\n".to_string() } else {
        let mut body = String::new();
        for r in &position_rows { body.push_str(&position_row(c, ctx, r, shown, hidden)?); }
        let money = if hidden { String::new() } else { format!("                <th>{}</th>\n                <th>{}</th>\n                <th>{}</th>\n                <th></th>\n", column("invested"), column("avg_buy"), column("price")) };
        format!("        <table class=\"table\">\n          <thead>\n            <tr>\n              <th class=\"text-left\">{}</th>\n              <th class=\"text-left\">{}</th>\n              <th class=\"text-left\">{}</th>\n              <th class=\"text-left\">{}</th>\n{money}              <th>{}</th>\n              <th>{}</th>\n            </tr>\n          </thead>\n          <tbody>\n            {body}\n          </tbody>\n        </table>\n",
            column("opened"), column("name"), column("type"), column("status"), ctx.t("data_labels.pnl"), column("hold"))
    };
    let first: Option<String> = c.query_row("SELECT MIN(transacted_at) FROM account_transactions WHERE user_id=?1", [owner], |r| r.get(0))?;
    let from = match ctx.params.query("from").filter(|v| !v.trim().is_empty()) { Some(v) => v.to_string(), None => first.map(|t| crate::codec::parse_time(&t).map(|t| t.date_naive().to_string()).map_err(|_| invalid())).transpose()?.unwrap_or_default() };
    let to = match ctx.params.query("to").filter(|v| !v.trim().is_empty()) { Some(v) => v.to_string(), None => ctx.app.now().date_naive().to_string() };
    let hidden_scope = ctx.params.query("exchange_id").filter(|s| !s.trim().is_empty()).map(|s| format!("        <input type=\"hidden\" name=\"exchange_id\" id=\"exchange_id\" value=\"{}\" />\n", escape(s))).unwrap_or_default();
    let filters_action = "segmented:change->tracker--record#mark segmented:change->order-filter#filter segmented:change->tracker--record#hold";
    Ok(format!("<div class=\"tracker-record\" data-controller=\"tracker--record\">\n  <div class=\"tracker-record__switch filters\" data-action=\"segmented:change->tracker--record#mark segmented:change->tracker--record#show segmented:change->tracker--record#hold\">\n    {switch}\n  </div>\n\n  <div class=\"tracker-record__pane tracker-record__pane--off\" data-pane=\"tx\"\n       data-controller=\"order-filter\" data-tracker--record-target=\"pane\">\n    <div class=\"tracker-record__filters filters\" data-action=\"{filters_action}\">\n{}    </div>\n    <div class=\"tracker-record__table widget widget--table\" data-controller=\"table-fit\">\n{tx_table}    </div>\n  </div>\n\n  <div class=\"tracker-record__pane\" data-pane=\"pos\"\n       data-controller=\"order-filter\" data-tracker--record-target=\"pane\">\n    <div class=\"tracker-record__filters filters\" data-action=\"{filters_action}\">\n{}    </div>\n    <div class=\"tracker-record__table widget widget--table tracker-positions\" data-controller=\"table-fit\">\n{positions_table}    </div>\n  </div>\n\n  <div class=\"tracker-record__actions\">\n    <form class=\"tracker-date-range\" data-controller=\"form--submit\" action=\"{}\" accept-charset=\"UTF-8\" method=\"get\">\n{hidden_scope}      <input type=\"date\" name=\"from\" value=\"{}\" class=\"sinput\" data-action=\"change->form--submit#submit\">\n      <input type=\"date\" name=\"to\" value=\"{}\" class=\"sinput\" data-action=\"change->form--submit#submit\">\n</form>  </div>\n</div>\n",
        filter(ctx.t("tracker.columns.type"), types)?, filter(ctx.t("tracker.columns.status"), statuses)?, escape(&ctx.path("/tracker")), escape(&from), escape(&to)))
}
