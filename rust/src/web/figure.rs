//! Read-only page figures. All decimals, including formatting, stay inside one budget and thread.
use crate::figures::{at::At, budget, chart, db::{self, Subject}, live, market::MarketData, num::Num, pair, walk::{self, Metrics}, FiguresError};
use rusqlite::Connection;
use serde_json::{json, Map, Value};

mod holdings;
pub(crate) use holdings::redeploy_offer;
pub use plot::NOT_CHARTED;
mod plot;
mod headline;
pub mod service;
pub mod loading;
pub mod warm;
use crate::figures::totals;

pub const NO_VALUE: &str = "<span class=\"no-value\">—</span>";
fn colour(n: &Num) -> &'static str { if n.is_negative() { "text-danger" } else if n.is_positive() { "text-success" } else { "" } }
fn precision(n: &Num, digits: usize, grouping: bool) -> Result<String, FiguresError> {
    let rounded = n.to_d()?.round(digits as i64)?.to_s_f();
    let (whole, fraction) = rounded.split_once('.').unwrap_or((&rounded, ""));
    let sign = if whole.starts_with('-') { "-" } else { "" };
    let unsigned = whole.strip_prefix('-').unwrap_or(whole);
    let mut out = sign.to_string();
    for (i, b) in unsigned.bytes().enumerate() {
        if grouping && i > 0 && (unsigned.len() - i).is_multiple_of(3) { out.push(','); }
        out.push(char::from(b));
    }
    if digits > 0 { out.push('.'); out.push_str(&format!("{fraction:0<digits$}")); }
    Ok(out)
}
fn percent(n: &Num, digits: usize) -> Result<String, FiguresError> {
    Ok(format!("{:.*}%", digits, n.mul(&Num::Int(100))?.to_f()))
}
/// Denomination::UNITS: the symbol of each currency an account may be shown in; any other is its code.
fn unit(currency: &str) -> &str {
    match currency { "USD" => "$", "EUR" => "€", "GBP" => "£", "CHF" => "Fr.", "PLN" => "zł", other => other }
}
/// Denomination::SUFFIXED: written after the amount in the countries that use it.
fn suffixed(currency: &str) -> bool { matches!(currency, "CHF" | "PLN") }
/// The thousands mark Rails writes in the headline's attribute: `number.currency.format.delimiter`, falling back to
/// `number.format.delimiter`, then ','. Measured on every locale Rails ships (fallbacks on, no rails-i18n gem, no number
/// keys in config/locales): ',' in each, so there is no table to keep.
const DELIMITER: &str = ",";
/// The account's display currency (User#denomination): the conversion is a held one, and when it cannot be had the
/// figures are shown in dollars (Denomination.for: "a złoty sign on a dollar figure would be a lie").
fn denomination(c: &Connection, market: &dyn MarketData, display_currency: &str) -> Result<totals::Denomination, FiguresError> {
    match totals::denomination(c, market, display_currency) {
        Err(FiguresError::NotComputed(reason)) if reason == FX_UNAVAILABLE => Ok(totals::Denomination { currency: "USD".into(), rate: crate::figures::dec::Dec::one() }),
        other => other,
    }
}
const FX_UNAVAILABLE: &str = "Currency conversion unavailable";
/// A figure that needed a conversion that cannot be had is absent, as Rails' is (a Failure, nil): never a page's error.
fn without_fx<T>(result: Result<Option<T>, FiguresError>) -> Result<Option<T>, FiguresError> {
    match result { Err(FiguresError::NotComputed(reason)) if reason == FX_UNAVAILABLE => Ok(None), other => other }
}
/// Denomination#format: a USD figure in the account's currency (`usd.to_d * rate`), the unit in <small> before the figure
/// or after it, as number_to_currency lays it out: the negative layout only for a loss of at least half the last digit
/// shown (NumberToCurrencyConverter: `abs * 10**precision >= 0.5`), so a loss that rounds to nothing is written as one.
fn shown(d: &totals::Denomination, usd: &Num, digits: usize) -> Result<String, FiguresError> {
    let amount = (&usd.to_d()? * &d.rate)?;
    // A negative zero is written as zero (`abs`).
    let magnitude = if amount.is_negative() { (&crate::figures::dec::Dec::from_i64(0) - &amount)? } else if amount.is_zero() { crate::figures::dec::Dec::from_i64(0) } else { amount.clone() };
    let scale = crate::figures::dec::Dec::from_i64(10_i64.pow(digits as u32));
    let negative = amount.is_negative() && (&magnitude * &scale)? >= crate::figures::dec::Dec::strict("0.5")?;
    let text = precision(&Num::Dec(magnitude), digits, true)?;
    let unit = format!("<small>{}</small>", crate::web::i18n::escape(unit(&d.currency)));
    let laid = if suffixed(&d.currency) { format!("{text} {unit}") } else { format!("{unit}{text}") };
    Ok(if negative { format!("-{laid}") } else { laid })
}
/// `Denomination.new(currency, rate).format(usd, precision:)`, for the parity grid's boundary vectors.
#[doc(hidden)]
pub fn denominated(currency: &str, rate: &str, usd: &str, digits: usize) -> Result<String, FiguresError> {
    budget::within(|| shown(&totals::Denomination { currency: currency.into(), rate: crate::figures::dec::Dec::strict(rate)? }, &Num::Dec(crate::figures::dec::Dec::strict(usd)?), digits))
}
/// A bot type's class and its tile's kind (`dom_id`): `Bots::Signal` is `("Signal", "signal")`.
pub(crate) fn kind_of(class: Option<&str>) -> (&'static str, &'static str) {
    match class {
        Some("Bots::DcaIndex") => ("DcaIndex", "dca_index"),
        Some("Bots::DcaSingleAsset") => ("DcaSingleAsset", "dca_single_asset"),
        Some("Bots::Signal") => ("Signal", "signal"),
        _ => ("DcaMultiAsset", "dca_multi_asset"),
    }
}
/// A bot whose figures raised (a transient read): empty targets, as a failed fill's (`loading::render`): no widget and no
/// loader left waiting, and the next fill's publication replaces them.
fn raised_targets(id: i64, kind: &str) -> Value {
    json!({ "tile": format!("<div id=\"pnl_bots_{kind}_{id}\"></div>"), "metrics": "<div id=\"metrics\"></div>",
        "chart": plot::NOT_CHARTED, "missing": [], "cached": false, "chart_cached": false })
}
/// NumbersHelper#format_value: below one, rounded to `max_decimals`, its trailing zeros dropped (a bare zero stays), printed
/// as Ruby prints it (a Float's exponent form included); from one on, two decimals.
pub(crate) fn format_value(value: &Num, max_decimals: u8) -> Result<String, crate::figures::num::NumError> {
    let abs = if value.is_negative() { Num::Int(0).sub(value)? } else { value.clone() };
    if abs.lt(&Num::Int(1))? {
        let text = match value {
            Num::Int(i) => i.to_string(),
            Num::Dec(d) => d.round(i64::from(max_decimals))?.to_s_f(),
            Num::Float(f) => crate::figures::num::float_to_s(crate::web::format::float_round(*f, i64::from(max_decimals))),
        };
        let trimmed = text.strip_suffix('0').map(|_| text.trim_end_matches('0').trim_end_matches('.')).unwrap_or(&text);
        Ok(if trimmed.is_empty() { "0".into() } else { trimmed.to_string() })
    } else {
        Ok(format!("{:.2}", value.to_f()))
    }
}
/// A figure as `<%= %>` prints it: an Integer, a BigDecimal (`to_s('F')`) or a Float.
pub(crate) fn printed(value: &Num) -> String {
    match value { Num::Int(i) => i.to_string(), Num::Dec(d) => d.to_s_f(), Num::Float(f) => crate::figures::num::float_to_s(*f) }
}
fn tile(id: i64, kind: &str, m: &Metrics, unavailable: bool, profit: Option<&Num>, d: &totals::Denomination) -> Result<String, FiguresError> {
    let mut body = String::new();
    // Figures that cannot be stated leave the tile without them: no widget, no dash.
    if let Some(pnl) = m.pnl.as_ref().filter(|_| !unavailable) {
        body = format!("<div class=\"bot-tile__pnl {}\">\n<span class=\"pnl-percent\">{}</span>\n", colour(pnl), percent(pnl, 2)?);
        // Bot#profit_in_usd in the account's currency; absent with balances hidden or without a rate, as in Rails.
        if let Some(profit) = profit {
            body.push_str(&format!("<span class=\"pnl-amount\">{}{}</span>\n", if profit.is_positive() { "+" } else { "" }, shown(d, profit, 2)?));
        }
        body.push_str("</div>\n");
    }
    Ok(format!("<div id=\"pnl_bots_{kind}_{id}\">\n{body}</div>\n"))
}
pub(crate) fn missing(s: &Subject, m: &Metrics, market: &dyn MarketData) -> Result<Vec<String>, FiguresError> {
    if m.asset_breakdown.is_empty() { return Ok(vec![]); }
    // Quarantined or unresolved splits leave ledger prices in the core result. Keep known
    // quantities and costs, but withhold every price-dependent value as for a missing price.
    if m.prices_stale {
        return Ok(m.asset_breakdown.iter().filter(|(_, holding)| holding.amount.is_positive()).map(|(key, _)| key.clone()).collect());
    }
    let symbols = s.tickers.iter().map(|t| t.ticker.clone()).collect::<Vec<_>>();
    let prices = market.prices(&live::venue(s)?, &symbols);
    let mut keys = vec![];
    for (key, holding) in &m.asset_breakdown {
        if !holding.amount.is_positive() { continue; }
        let ticker = live::ticker_for_key(s, m, key);
        let omitted = ticker.is_none() || prices.as_ref().is_ok_and(|prices| ticker.is_some_and(|t| !prices.iter().any(|(code,_)| code == &t.ticker)));
        if omitted { keys.push(key.clone()); }
    }
    Ok(keys)
}

/// One bot's figures as the account's totals read them: `marked` None is no track in the history (its chart raised), and
/// `read` false none in a page's read of it.
struct Done { subject: Subject, live: Metrics, marked: Option<Metrics>, missing: Vec<String>, read: bool }

/// The composition metrics partial with current prices and no loading placeholders or session token.
/// A settings callback needs only this panel, so it never asks for candles or account totals.
pub fn metrics_panel(c: &Connection, id: i64, market: &dyn MarketData, now: At, locale: &str, prefix: &str) -> Result<String, FiguresError> {
    budget::within(|| {
        let subject = Subject::load(c, id)?;
        let user = db::user(c, subject.bot.user_id)?;
        let walked = walk::metrics(c, &subject, now)?;
        let metrics = live::live(c, &subject, &walked, market, now)?;
        let missing = missing(&subject, &metrics, market)?;
        holdings::render(c, &subject, &metrics, &missing, user.hide_balances, locale, &mut holdings::Token::Value(""), prefix, now)
    })
}

#[allow(clippy::too_many_arguments)]
pub fn account(c: &Connection, user_id: i64, market: &dyn MarketData, now: At, locale: &str, csrf: &str, prefix: &str) -> Result<Value, FiguresError> {
    Ok(computed(c, user_id, market, now, locale, Some(csrf), prefix)?.0)
}
/// Where each bot's metrics take a form token: (bot id, byte offsets into its `metrics`).
pub(crate) type Slots = Vec<(String, Vec<usize>)>;
/// `account` with every form token left empty and where it goes recorded, for figures kept across requests.
pub(crate) fn kept_account(c: &Connection, user_id: i64, market: &dyn MarketData, now: At, locale: &str, prefix: &str) -> Result<(Value, Slots), FiguresError> {
    computed(c, user_id, market, now, locale, None, prefix)
}
/// `csrf`: the token, or None to record where it goes.
fn computed(c: &Connection, user_id: i64, market: &dyn MarketData, now: At, locale: &str, csrf: Option<&str>, prefix: &str) -> Result<(Value, Slots), FiguresError> {
    budget::within(|| {
        let mut slots = vec![];
        let mut bots = Map::new();
        let mut computed = vec![];
        let user = db::user(c, user_id)?;
        // One rate into the account's currency, and one per quote currency, for the whole page (BotsController#index).
        let shown_in = denomination(c, market, &user.display_currency)?;
        let mut rates = totals::Rates::default();
        // A bot whose live prices raised (a transient read) is left out, as Rails' metrics job for that bot raises and is
        // retried: its panels stay loading while every other bot's figures stand. The headline is held back while such a
        // bot has orders (User#global_pnl_snapshot: loading while a traded bot has no figures).
        let mut withheld = false;
        let mut raised = vec![];
        let mut history_cold = false;
        // A pair bot's marked chart that raised (Rails: its live figures cached and broadcast, the chart retried): the
        // headline is shown without history.
        let mut marked_raised = false;
        for (id, class) in db::account_bots(c, user_id)? {
            let (_, kind) = kind_of(class.as_deref());
            if class.as_deref().is_some_and(pair::is_pair) {
                // Bots::DcaSingleAsset::Measurable, which Bots::Signal includes: the walk, the live pass and the marked chart,
                // each saying whether Rails caches it.
                let live = match pair::metrics(c, id, now).and_then(|walked| pair::live(c, &walked, market, now)) {
                    Err(FiguresError::Raised(_)) => {
                        bots.insert(id.to_string(), raised_targets(id, kind));
                        withheld |= !db::orders(c, id)?.is_empty(); raised.push(id); continue;
                    }
                    other => other?,
                };
                let (live, cached) = live;
                let marked = match pair::marked_at(&live, market, now) { Err(FiguresError::Raised(_)) => None, other => Some(other?) };
                marked_raised |= marked.is_none();
                let chart_cache = marked.as_ref().is_some_and(|(_, gridded)| *gridded);
                let metrics = holdings::render_pair(c, &live, user.hide_balances, locale, prefix, now)?;
                let profit = if user.hide_balances { None } else {
                    match totals::profit_in_usd(c, market, &mut rates, live.subject.quote.as_deref(), &live.metrics) {
                        Err(FiguresError::NotComputed(reason)) if reason == "quote currency unavailable" => None,
                        other => without_fx(other)?,
                    }
                };
                // D3 (3.0, Rust only): a stale pair's chart is not drawn, as a basket's is not: Rails' broadcast draws it.
                let chart = match &marked { Some((m, _)) => plot::render(c, &live.subject, &m.metrics, &[], user.hide_balances, locale, &user.time_zone, true)?, None => plot::NOT_CHARTED.into() };
                bots.insert(id.to_string(), json!({ "tile": tile(id, kind, &live.metrics, live.metrics.prices_stale, profit.as_ref(), &shown_in)?, "metrics": metrics, "chart": chart, "missing": [],
                    "cached": cached,
                    // The page draws a chart only beside cached figures (@chart_metrics is read only unless @metrics_loading).
                    "chart_cached": cached && chart_cache }));
                history_cold |= cached && !chart_cache;
                // User::PnlHistory#marked_metrics (a page's read) takes the candles cache first, and nothing when the prices
                // cache has no labels either: a bot with neither cached is no track there. The live pass reads its chart.
                computed.push(Done { subject: live.subject, live: live.metrics, marked: marked.map(|(m, _)| m.metrics), missing: vec![], read: cached || chart_cache });
                continue;
            }
            let subject = Subject::load(c, id)?;
            let walked = walk::metrics(c, &subject, now)?;
            let marked_live = match live::live(c, &subject, &walked, market, now) {
                Err(FiguresError::Raised(_)) => {
                    bots.insert(id.to_string(), raised_targets(id, kind));
                    withheld |= !subject.orders.is_empty(); raised.push(id); continue;
                }
                other => other?,
            };
            let missing = missing(&subject, &marked_live, market)?;
            let (marked, gridded) = chart::marked_at(c, &subject, &marked_live, market, now)?;
            let mut offsets = vec![];
            let mut token = match csrf { None => holdings::Token::Slots(&mut offsets), Some(token) => holdings::Token::Value(token) };
            let metrics = holdings::render(c, &subject, &marked_live, &missing, user.hide_balances, locale, &mut token, prefix, now)?;
            if !offsets.is_empty() { slots.push((id.to_string(), offsets)); }
            // The amount is all a rate was needed for: with balances hidden there is none to convert (BotsController#index).
            let profit = if user.hide_balances { None } else {
                match totals::profit_in_usd(c, market, &mut rates, subject.quote.as_deref(), &marked_live) {
                    Err(FiguresError::NotComputed(reason)) if reason == "quote currency unavailable" => None,
                    other => without_fx(other)?,
                }
            };
            bots.insert(id.to_string(), json!({ "tile": tile(id, kind, &marked_live, marked_live.prices_stale || !missing.is_empty(), profit.as_ref(), &shown_in)?, "metrics": metrics, "chart": plot::render(c, &subject, &marked, &missing, user.hide_balances, locale, &user.time_zone, false)?, "missing": missing,
                // What Rails would find in its cache (Measurable#metrics_with_current_prices): stale figures, or ones with no
                // chart point yet, are never written, so its bot page reads them as still loading.
                "cached": !marked_live.prices_stale && !marked_live.chart.labels.is_empty(),
                // And the marked chart (metrics_with_current_prices_and_candles) only once a holding had candles to mark it
                // with (`chart::marked_at`).
                "chart_cached": !marked_live.prices_stale && !marked_live.chart.labels.is_empty() && gridded }));
            // User::PnlHistory#marked_metrics: figures cached without a marked chart are a cold track, and one cold track
            // is the whole account's history waiting.
            history_cold |= !marked_live.prices_stale && !marked_live.chart.labels.is_empty() && !gridded;
            computed.push(Done { subject, live: marked_live, marked: Some(marked), missing, read: true });
        }
        let unavailable = computed.iter().any(|d| d.live.prices_stale || !d.missing.is_empty() || d.marked.as_ref().is_some_and(|m| !m.chart_omitted.is_empty()));
        // (marked, as a page reads them)
        let parts = |marked: bool, page: bool| computed.iter().map(|d| {
            let figures = if !marked { Some(&d.live) } else if page && !d.read { None } else { d.marked.as_ref() };
            // No marked track: no part of the history, as Rails skips it.
            totals::Part { bot_id: d.subject.bot.id, quote: d.subject.quote.as_deref(), traded: figures.is_some() && !d.subject.orders.is_empty(), figures: Ok(figures) }
        }).collect::<Vec<_>>();
        let pnl = without_fx(totals::global_pnl(c,market,&mut totals::Rates::default(),&parts(false,false)))?;
        let history = |page: bool| -> Result<_, FiguresError> { if marked_raised { Ok(None) } else { without_fx(totals::pnl_history(c,market,&mut totals::Rates::default(),&parts(true,page)).map(|h|h.result)) } };
        let (history, page_history) = (history(false)?, if computed.iter().all(|d| d.read) { None } else { Some(history(true)?) });
        let mut out = json!({"bots": bots});
        if !raised.is_empty() { out["raised"] = json!(raised); }
        // A pair's chart that raised is retried, as Rails retries its metrics job: these figures last the price's minute.
        if marked_raised { out["retry"] = Value::Bool(true); }
        // A traded bot without figures: no total without it (User#global_pnl_snapshot), and its target stays empty for the
        // next fill's publication.
        if withheld { out["account"] = Value::String("<div id=\"global-pnl\"></div>".into()); }
        if !withheld {
            // The publication is Rails' live pass (PnlHistory.snapshot(live: true)): its history reads every bot's chart as
            // computed, marked or not. A page reads Rails' caches: while a bot's marked chart is cold, its headline has no
            // history yet, and the page asks for the live pass (@global_pnl_history_loading).
            out["account"] = Value::String(headline::render(pnl.as_ref(),history.as_ref(),unavailable,user.hide_balances,&shown_in)?);
            if history_cold && !marked_raised {
                out["account_page"] = Value::String(headline::render(pnl.as_ref(),None,unavailable,user.hide_balances,&shown_in)?);
                out["history_loading"] = Value::Bool(true);
            }
            // A track the page's read does not have (a pair with nothing cached): its history is the others'.
            else if let Some(page) = page_history.filter(|page| *page != history) { out["account_page"] = Value::String(headline::render(pnl.as_ref(),page.as_ref(),unavailable,user.hide_balances,&shown_in)?); }
        }
        let bytes = out.to_string().len();
        if bytes > 8 * 1024 * 1024 { return Err(FiguresError::NotComputed(crate::figures::OVER_BUDGET.into())); }
        budget::charge(bytes as u64,0)?;
        budget::check()?;
        Ok((out, slots))
    })
}

/// The payload boundary for 3b-2b-2: same targets and unsigned stream names as the model broadcasts.
/// This creates payloads only; delivery and the authenticated broadcast routes remain a separate change.
pub fn streams(c:&Connection,user:i64,rendered:&Value)->Result<Vec<(String,String)>,FiguresError>{
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD,Engine};
    let mut out=vec![];
    for (id,class) in db::account_bots(c,user)? {
        let (class,kind)=kind_of(class.as_deref());
        let page=format!("{}:bot_updates",URL_SAFE_NO_PAD.encode(format!("gid://deltabadger/Bots::{class}/{id}")));
        for (part,target,stream) in [("tile",format!("pnl_bots_{kind}_{id}"),format!("user_{user}:bot_updates")),("metrics","metrics".into(),page.clone()),("chart","chart".into(),page)]{
            if let Some(html)=rendered["bots"][id.to_string()][part].as_str(){out.push((stream,crate::web::turbo::stream("replace",&target,html)));}
        }
    }
    if let Some(html)=rendered["account"].as_str(){out.push((format!("user_{user}:bot_updates"),crate::web::turbo::stream("replace","global-pnl",html)));}
    Ok(out)
}
