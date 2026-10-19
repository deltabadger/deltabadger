//! Read-only page figures. All decimals, including formatting, stay inside one budget and thread.
use crate::figures::{at::At, budget, chart, db::{self, Subject}, live, market::MarketData, num::Num, walk::{self, Metrics}, FiguresError};
use rusqlite::Connection;
use serde_json::{json, Map, Value};

mod holdings;
pub(crate) use holdings::redeploy_offer;
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
fn tile(s: &Subject, m: &Metrics, unavailable: bool, profit: Option<&Num>, d: &totals::Denomination) -> Result<String, FiguresError> {
    let kind = if s.bot.kind == db::Kind::Basket { "dca_multi_asset" } else { "dca_index" };
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
    Ok(format!("<div id=\"pnl_bots_{kind}_{}\">\n{body}</div>\n", s.bot.id))
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
        for (id, _) in db::account_bots(c, user_id)? {
            let subject = Subject::load(c, id)?;
            let walked = walk::metrics(c, &subject, now)?;
            let marked_live = live::live(c, &subject, &walked, market, now)?;
            let missing = missing(&subject, &marked_live, market)?;
            let marked = chart::marked(c, &subject, &marked_live, market, now)?;
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
            bots.insert(id.to_string(), json!({ "tile": tile(&subject, &marked_live, marked_live.prices_stale || !missing.is_empty(), profit.as_ref(), &shown_in)?, "metrics": metrics, "chart": plot::render(c, &subject, &marked, &missing, user.hide_balances, locale, &user.time_zone)?, "missing": missing,
                // What Rails would find in its cache (Measurable#metrics_with_current_prices): stale figures, or ones with no
                // chart point yet, are never written, so its bot page reads them as still loading.
                "cached": !marked_live.prices_stale && !marked_live.chart.labels.is_empty() }));
            computed.push((subject, marked_live, marked, missing));
        }
        let unavailable = computed.iter().any(|(_,live,m,missing)| live.prices_stale || !missing.is_empty() || !m.chart_omitted.is_empty());
        let parts = |marked: bool| computed.iter().map(|(s,live,chart,_)| totals::Part { bot_id:s.bot.id, quote:s.quote.as_deref(),traded:!s.orders.is_empty(),figures:Ok(Some(if marked { chart } else { live })) }).collect::<Vec<_>>();
        let pnl = without_fx(totals::global_pnl(c,market,&mut totals::Rates::default(),&parts(false)))?;
        let history = without_fx(totals::pnl_history(c,market,&mut totals::Rates::default(),&parts(true)).map(|h|h.result))?;
        let out = json!({"bots": bots, "account": headline::render(pnl.as_ref(),history.as_ref(),unavailable,user.hide_balances,&shown_in)?});
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
    for (id,_) in db::account_bots(c,user)? {
        let bot=db::bot(c,id)?;
        let (class,kind)=if bot.kind==db::Kind::Index{("DcaIndex","dca_index")}else{("DcaMultiAsset","dca_multi_asset")};
        let page=format!("{}:bot_updates",URL_SAFE_NO_PAD.encode(format!("gid://deltabadger/Bots::{class}/{id}")));
        for (part,target,stream) in [("tile",format!("pnl_bots_{kind}_{id}"),format!("user_{user}:bot_updates")),("metrics","metrics".into(),page.clone()),("chart","chart".into(),page)]{
            if let Some(html)=rendered["bots"][id.to_string()][part].as_str(){out.push((stream,crate::web::turbo::stream("replace",&target,html)));}
        }
    }
    if let Some(html)=rendered["account"].as_str(){out.push((format!("user_{user}:bot_updates"),crate::web::turbo::stream("replace","global-pnl",html)));}
    Ok(out)
}
