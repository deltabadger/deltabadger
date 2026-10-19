//! GET /tracker/history_gap?exchange_id=N (Rust only, 3.0): why a venue's history is incomplete, what the tracker
//! found, and how to import the rest. The evidence is the stored gap (tracker::gaps), read as the page reads it; the
//! steps and the import button depend only on whether the venue has a reading key now.
use super::{auth, layout, App, Ctx, WebError};
use crate::figures::{dec::Dec, totals::Denomination};
use crate::tracker::{cache, gaps, import::run};
use crate::web::i18n::{self, escape, Arg};
use askama::Template;
use axum::{extract::{Extension, State}, http::header, response::{IntoResponse, Response}};
use rusqlite::{Connection, OptionalExtension};

/// The CSV examples: an old swap, a recent Convert (or HF) swap, a deposit on its own. `{v}` is the venue's name_id.
const HEADER: &str = "date,type,base_currency,base_amount,quote_currency,quote_amount,fee_currency,fee_amount,exchange,tx_id,group_id,description";
const EXAMPLES: [(&str, &[&str]); 3] = [
    ("swap", &["2022-05-10T12:00:00Z,swap_out,BTC,0.5,,,,,{v},,{v}-2022-05-10,", "2022-05-10T12:00:00Z,swap_in,ETH,7.4,,,,,{v},,{v}-2022-05-10,"]),
    ("convert", &["2026-09-20T08:00:00Z,swap_out,USDT,500,,,,,{v},,{v}-convert-1,", "2026-09-20T08:00:00Z,swap_in,SOL,3.1,,,,,{v},,{v}-convert-1,"]),
    ("deposit", &["2023-01-05T10:00:00Z,deposit,BTC,0.25,,,,,{v},,,"]),
];

/// The rows each example puts in a file, for the test that imports them.
#[doc(hidden)]
pub fn examples(name_id: &str) -> Vec<String> {
    EXAMPLES.iter().flat_map(|(_, rows)| rows.iter().map(|r| r.replace("{v}", name_id))).collect()
}
#[doc(hidden)]
pub const CSV_HEADER: &str = HEADER;

struct Example { caption: String, rows: String }
struct KeyFix { href: String, label: String }

#[derive(Template)]
#[template(path = "tracker/history_gap.html")]
struct Modal<'a> { v: &'a Ctx, title: String, why: Vec<String>, evidence: Vec<String>, steps: Vec<String>, examples: Vec<Example>, header: &'static str, eligible: bool, cannot: String, fixes: Vec<KeyFix>, import: String }

pub async fn show(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(owner) = ctx.user().map(|u| u.id) else { return Ok(auth::unauthenticated(&ctx)) };
    app.db(move |c| super::read::only(c, |c| render(c, &ctx, owner))).await
}

/// The modal for the signed-in `owner`, in one read: 404 for an exchange the user has neither a key nor rows on.
#[doc(hidden)]
pub fn render(c: &Connection, ctx: &Ctx, owner: i64) -> Result<Response, WebError> {
    if c.is_autocommit() { let tx = c.unchecked_transaction()?; let out = render(&tx, ctx, owner)?; tx.commit()?; return Ok(out); }
    let Some(exchange) = ctx.params.query("exchange_id").and_then(crate::web::bot::id_from_path) else { return Ok(layout::missing()) };
    // The venue, whether it has a trading key of the user's (the key-fix links), and whether the user has it at all.
    let venue: Option<(String, String, Option<bool>, bool)> = c.query_row(
        "SELECT e.name, e.type, (SELECT MAX(key_type = 0) FROM api_keys WHERE user_id = ?1 AND exchange_id = e.id AND key_type != 1), \
         EXISTS(SELECT 1 FROM api_keys WHERE user_id = ?1 AND exchange_id = e.id) OR EXISTS(SELECT 1 FROM account_transactions WHERE user_id = ?1 AND exchange_id = e.id) \
         FROM exchanges e WHERE e.id = ?2", rusqlite::params![owner, exchange], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional()?;
    let Some((name, kind, trading, _)) = venue.filter(|v| v.3) else { return Ok(layout::missing()) };
    let reading = run::reading(c, owner).map_err(|e| WebError::Config(e.0))?;
    let entry = crate::figures::budget::within(|| cache::read_published(c, owner, ctx.app.now())).map_err(|_| super::row::invalid())?;
    let from = match &entry { cache::Published::Warm { from, .. } => Some(from.clone()), _ => None };
    let found = gaps::read(c, owner, Some(exchange), from.as_ref(), Some(&reading)).unwrap_or_else(|e| {
        crate::engine::log(&format!("[tracker] history gap for user {owner} not read: {}", crate::tracker::jobs::message(e))); vec![]
    });
    let hidden = ctx.user().ok_or_else(super::row::invalid)?.hide_balances;
    let named = [("exchange", Arg::Text(&name))];
    let t = |key: &str| i18n::t(ctx.locale, &format!("tracker.history_gap.{key}"), &named);
    let mut evidence = vec![];
    if let Some(gap) = found.first() {
        let usd = Denomination { currency: "USD".into(), rate: Dec::one() };
        let den = super::fx::held(&ctx.app, c, owner)?.denomination(c, owner, ctx.app.now().timestamp())?.unwrap_or(usd);
        for note in &gap.notes { evidence.push(escape(&super::populated::note_text(ctx, note, &den, hidden)?)); }
    }
    let (why, steps): (&[&str], &[&str]) = match kind.as_str() {
        crate::venue::kucoin::KUCOIN => (&["why_kucoin_reach", "why_kucoin_hf_convert", "why_kucoin_transfers"], &["steps_kucoin_1", "steps_kucoin_2", "steps_kucoin_3"]),
        crate::venue::binance::BINANCE => (&["why_binance"], &["steps_binance_1", "steps_binance_2", "steps_binance_3"]),
        _ => (&["why_other"], &["steps_other_1", "steps_other_2"]),
    };
    let name_id = crate::tracker::rows::name_id(&kind);
    let examples = if kind == crate::venue::binance::BINANCE { vec![] } else {
        EXAMPLES.iter().map(|(caption, rows)| Example { caption: t(&format!("csv_example_caption.{caption}")), rows: escape(&rows.iter().map(|r| r.replace("{v}", &name_id)).collect::<Vec<_>>().join("\n")) }).collect()
    };
    let eligible = reading.iter().any(|k| k.exchange_id == exchange);
    // As tracker/_key_fix_buttons picks them: a trading key is replaced, or a tracker key added beside it; a tracker
    // key is replaced; a venue with no key gets one.
    let link = |key_type: &str, label: &str| KeyFix { href: escape(&ctx.path(&format!("/tracker/add_api_key/new?exchange_id={exchange}&key_type={key_type}"))), label: i18n::t(ctx.locale, label, &named) };
    let fixes = if eligible { vec![] } else {
        match trading {
            Some(true) => vec![link("trading", "tracker.replace_trading_key"), link("read_only", "tracker.add_tracker_key")],
            Some(false) => vec![link("read_only", "tracker.replace_tracker_key")],
            None => vec![link("read_only", "tracker.add_tracker_key")],
        }
    };
    let body = Modal { v: ctx, title: t("title"), why: why.iter().map(|k| t(k)).collect(), evidence, steps: steps.iter().map(|k| t(k)).collect(), examples, header: HEADER,
                       eligible, cannot: t("cannot_import"), fixes, import: escape(&ctx.path("/tracker/import/new")) }.render()?;
    Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response())
}
