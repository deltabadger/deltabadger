//! Bot::WashSaleGuard: the account's wash-sale locks as the trading legs read and write them. A lock forbids buying the
//! asset, never selling it. Everything here is a no-op while the user's window is 0 days (the rule off or undecided).
//! Datetimes are stored and compared as Rails' text (`codec::format_time`); the deadline day is UTC, the app's zone.
use super::model::{self, Bot, Level, Ticker};
use super::EngineError;
use crate::codec::format_time;
use crate::figures::{at::At, db::Subject, dec::Dec, lots, walk, FiguresError};
use crate::ruby::BigDec;
use chrono::{DateTime, Days, NaiveDate, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use std::collections::HashMap;

fn fig(e: FiguresError) -> EngineError { EngineError::Data(format!("{e:?}")) }

/// User#wash_sale_days.
pub fn days(c: &Connection, user_id: i64) -> Result<u64, EngineError> { crate::tracker::wash::days(c, user_id).map_err(fig) }

/// User#locked_asset_ids(now): the assets whose effective deadline is still ahead (`buy_locked_until > now`, strict).
/// Empty while the window is 0 days, whatever rows stand; the clock is read only past that, so an account without the
/// rule reads no more of it than before.
pub fn locked(c: &Connection, user_id: i64, clock: &dyn super::Clock) -> Result<Vec<i64>, EngineError> {
    if days(c, user_id)? == 0 { return Ok(vec![]); }
    let mut s = c.prepare("SELECT asset_id FROM wash_sale_locks WHERE user_id = ?1 AND buy_locked_until > ?2 ORDER BY asset_id")?;
    let out = s.query_map(params![user_id, format_time(clock.now())], |r| r.get(0))?.collect::<Result<_, _>>()?;
    Ok(out)
}

/// #lock_deadline(from:): buying resumes at the start of the day `days + 1` after `from`.
fn deadline_day(from: NaiveDate, days: u64) -> Result<NaiveDate, EngineError> {
    from.checked_add_days(Days::new(days + 1)).ok_or_else(|| EngineError::Data("a wash-sale deadline out of range".into()))
}
fn deadline(from: NaiveDate, days: u64) -> Result<String, EngineError> {
    Ok(format_time(deadline_day(from, days)?.and_time(chrono::NaiveTime::MIN).and_utc()))
}

/// What #lock_buying! returns: the effective deadline that stood before (as stored), and the token this placement wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct Claim { pub previous: Option<String>, pub token: String }

/// #lock_buying!(asset_id): the row created if missing (#wash_sale_lock_for), the effective deadline raised to the window
/// from `today` (never lowered) and the claim token replaced. None while the window is 0 days.
pub fn claim(c: &Connection, user_id: i64, asset_id: i64, today: NaiveDate, now: DateTime<Utc>) -> Result<Option<Claim>, EngineError> {
    let days = days(c, user_id)?;
    if days == 0 { return Ok(None); }
    let d = deadline(today, days)?;
    let at = format_time(now);
    c.execute("INSERT INTO wash_sale_locks (asset_id, user_id, created_at, updated_at) SELECT ?1, ?2, ?3, ?3 \
               WHERE NOT EXISTS (SELECT 1 FROM wash_sale_locks WHERE user_id = ?2 AND asset_id = ?1)", params![asset_id, user_id, at])?;
    let previous: Option<String> = c.query_row("SELECT buy_locked_until FROM wash_sale_locks WHERE user_id = ?1 AND asset_id = ?2",
                                               params![user_id, asset_id], |r| r.get(0))?;
    let token = uuid::Uuid::new_v4().simple().to_string()[..16].to_string(); // SecureRandom.hex(8)
    // update_columns: updated_at untouched.
    c.execute("UPDATE wash_sale_locks SET buy_locked_until = MAX(COALESCE(buy_locked_until, ?1), ?1), claim_token = ?2 WHERE user_id = ?3 AND asset_id = ?4",
              params![d, token, user_id, asset_id])?;
    Ok(Some(Claim { previous, token }))
}

/// #restore_buy_lock!: one statement, guarded by the claim's token (a foreign or newer claim makes it a no-op), floored at
/// what a fill confirmed.
pub fn restore(c: &Connection, user_id: i64, asset_id: i64, claim: &Claim) -> Result<(), EngineError> {
    c.execute("UPDATE wash_sale_locks SET buy_locked_until = COALESCE(MAX(confirmed_locked_until, ?1), confirmed_locked_until, ?1), claim_token = NULL \
               WHERE user_id = ?2 AND asset_id = ?3 AND claim_token = ?4", params![claim.previous, user_id, asset_id, claim.token])?;
    Ok(())
}

/// #extend_buy_lock!(asset_id:, from: today): WashSaleLock.confirm!(source: 'bot'). True when the effective lock moved.
pub fn confirm_from_fill(c: &Connection, user_id: i64, asset_id: i64, now: DateTime<Utc>) -> Result<bool, EngineError> {
    let days = days(c, user_id)?;
    if days == 0 { return Ok(false); }
    let mut locks = HashMap::new();
    if let Some(id) = c.query_row("SELECT id FROM wash_sale_locks WHERE user_id = ?1 AND asset_id = ?2", params![user_id, asset_id], |r| r.get(0)).optional()? {
        locks.insert(asset_id, id);
    }
    crate::tracker::wash::confirm(c, user_id, &mut locks, asset_id, now.date_naive(), days, "bot", now).map_err(fig)
}

/// #sell_at_loss?(order_data): whether selling `amount` for `quote` (the sized order, not the wire-rounded one) loses on
/// any FIFO lot it consumes, after this bot's resting sells of the asset took theirs. An unknown verdict is a loss, and
/// so is a holding recorded without an asset under one of the ticker's names. False while the window is 0 days.
pub fn sell_at_loss(c: &Connection, bot: &Bot, ticker: &Ticker, amount: &BigDec, quote: &BigDec, now: DateTime<Utc>) -> Result<bool, EngineError> {
    if days(c, bot.user_id)? == 0 { return Ok(false); }
    let at = At::from_utc(now).ok_or_else(|| EngineError::Data("the verdict's time is out of range".into()))?;
    let metrics = walk::metrics(c, &Subject::load(c, bot.id).map_err(fig)?, at).map_err(fig)?;
    let all_lots: &[(String, lots::Lots)] = metrics.walked.as_ref().map_or(&[], |w| &w.asset_lots);
    let lots_of = |key: &str| all_lots.iter().find(|(k, _)| k == key).map(|(_, l)| l);
    let num = |e| EngineError::Data(format!("{e:?}"));
    let spelling = crate::figures::db::base_spelling(&ticker.base_code).to_string();
    // #unresolved_lots_named?
    let names: Vec<String> = [&spelling, &ticker.base_symbol].into_iter().filter(|n| !n.trim().is_empty()).map(|n| n.to_uppercase()).collect();
    for (key, asset) in &metrics.key_assets {
        if asset.is_some() { continue; }
        let named = metrics.key_strings.iter().find(|(k, _)| k == key).is_some_and(|(_, s)| s.iter().any(|s| names.contains(&s.to_uppercase())));
        if named && lots_of(key).is_some_and(|l| lots::units(l).is_ok_and(|u| u.is_positive())) { return Ok(true); }
    }
    let mut held = metrics.key_for(ticker.base_asset_id).and_then(lots_of).cloned().unwrap_or_default();
    let base_names = serde_json::to_string(&[spelling, ticker.base_symbol.clone()]).expect("strings");
    let mut s = c.prepare("SELECT amount, amount_exec FROM transactions WHERE bot_id = ?1 AND status = 0 AND external_status IN (0, 1) AND side = 1 \
                           AND (base_asset_id = ?2 OR (base_asset_id IS NULL AND base IN (SELECT value FROM json_each(?3))))")?;
    let mut rows = s.query(params![bot.id, ticker.base_asset_id, base_names])?;
    let mut resting = Dec::zero();
    while let Some(r) = rows.next()? {
        let amount = Dec::from_sql(r.get_ref(0)?).map_err(num)?.unwrap_or_else(Dec::zero);
        let exec = Dec::from_sql(r.get_ref(1)?).map_err(num)?.unwrap_or_else(Dec::zero);
        let left = (&amount - &exec).map_err(num)?;
        if left.is_positive() { resting = (&resting + &left).map_err(num)?; }
    }
    lots::consume(&mut held, &resting).map_err(num)?;
    let (amount, quote) = (Dec::parse(&amount.to_s_f()).map_err(num)?, Dec::parse(&quote.to_s_f()).map_err(num)?);
    judged(&held, &amount, &quote).map_err(num)
}

/// `TaxLots.loss_in?(lots, amount, proceeds) != false`: an unknown verdict reads as a loss.
fn judged(held: &lots::Lots, amount: &Dec, quote: &Dec) -> Result<bool, crate::figures::num::NumError> {
    Ok(lots::loss_in(held, amount, quote)? != Some(false))
}

/// #log_wash_sale_lock: activity `wash_sale_locked` with the deadline actually on the row (an earlier sale's longer
/// window wins), as the last day locked.
pub fn log_locked(c: &Connection, bot: &Bot, asset_id: i64, now: DateTime<Utc>) -> Result<(), EngineError> {
    let until: Option<String> = c.query_row("SELECT buy_locked_until FROM wash_sale_locks WHERE user_id = ?1 AND asset_id = ?2",
                                            params![bot.user_id, asset_id], |r| r.get(0)).optional()?.flatten();
    let until = match until {
        Some(t) => crate::codec::parse_time(&t).map_err(EngineError::Arithmetic)?.date_naive(),
        None => deadline_day(now.date_naive(), days(c, bot.user_id)?)?,
    };
    let last = until.checked_sub_days(Days::new(1)).ok_or_else(|| EngineError::Data("a wash-sale deadline out of range".into()))?;
    let base: Option<String> = c.query_row("SELECT symbol FROM assets WHERE id = ?1", [asset_id], |r| r.get(0)).optional()?.flatten();
    model::log_activity(c, bot.id, "wash_sale_locked", Level::Info, json!({ "base": base, "until": last.format("%Y-%m-%d").to_string() }), now)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "'2026-01-01 00:00:00'";

    fn db(enabled: Option<bool>, jurisdiction: Option<&str>) -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../../tests/fixtures/settings_primary_schema.sql")).unwrap();
        c.execute(&format!("INSERT INTO users(id,email,encrypted_password,created_at,updated_at,wash_sale_enabled,wash_sale_jurisdiction) VALUES (1,'o@example.com','x',{T},{T},?1,?2)"),
                  params![enabled, jurisdiction]).unwrap();
        c.execute_batch(&format!("
            INSERT INTO exchanges(id,type,name,created_at,updated_at) VALUES (1,'Exchanges::Alpaca','Alpaca',{T},{T});
            INSERT INTO assets(id,category,external_id,symbol,name,created_at,updated_at) VALUES (1,'Fiat','usd','USD','US Dollar',{T},{T}),(2,'Stock','aaa','AAA','AAA Inc',{T},{T});
            INSERT INTO tickers(id,base,base_asset_id,base_decimals,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,created_at,updated_at)
                VALUES (1,'AAA',2,9,1,0,1,2,'USD',1,2,'AAA',{T},{T});
            INSERT INTO bots(id,user_id,exchange_id,type,status,label,settings,transient_data,created_at,updated_at) VALUES (1,1,1,'Bots::DcaMultiAsset',2,'b','{{}}','{{}}',{T},{T});
            INSERT INTO bot_index_assets(bot_id,asset_id,ticker_id,in_index,created_at,updated_at) VALUES (1,2,1,1,{T},{T});
        ")).unwrap();
        c
    }

    fn now(t: &str) -> DateTime<Utc> { crate::codec::parse_time(t).unwrap() }
    fn day(d: &str) -> NaiveDate { d.parse().unwrap() }
    fn row(c: &Connection) -> (Option<String>, Option<String>, Option<String>, String) {
        c.query_row("SELECT buy_locked_until, confirmed_locked_until, claim_token, source FROM wash_sale_locks WHERE user_id = 1 AND asset_id = 2",
                    [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap()
    }
    fn set(c: &Connection, buy: Option<&str>, confirmed: Option<&str>) {
        c.execute(&format!("INSERT INTO wash_sale_locks(asset_id,user_id,created_at,updated_at) SELECT 2,1,{T},{T} WHERE NOT EXISTS (SELECT 1 FROM wash_sale_locks)"), []).unwrap();
        c.execute("UPDATE wash_sale_locks SET buy_locked_until = ?1, confirmed_locked_until = ?2", params![buy, confirmed]).unwrap();
    }

    /// The window by jurisdiction (US 30, GB 30, IE 28, blank → US, unknown → 0, rule off or undecided → 0); the deadline is
    /// the start of day N+1 in UTC, written as Rails' text; a lock is live strictly before it.
    #[test]
    fn deadline_arithmetic_and_the_strict_boundary() {
        for (j, until) in [(Some("US"), "2026-10-21 00:00:00"), (Some("GB"), "2026-10-21 00:00:00"), (Some("IE"), "2026-10-19 00:00:00"), (Some(""), "2026-10-21 00:00:00"), (None, "2026-10-21 00:00:00")] {
            let c = db(Some(true), j);
            let claim = claim(&c, 1, 2, day("2026-09-20"), now("2026-09-20T23:30:00Z")).unwrap().unwrap();
            assert_eq!(claim.previous, None);
            assert_eq!(row(&c).0.as_deref(), Some(until), "{j:?}");
            assert_eq!(row(&c).3, "bot");
            let before = crate::codec::parse_time(until).unwrap() - chrono::Duration::microseconds(1);
            assert_eq!(locked(&c, 1, &super::super::FixedClock(before)).unwrap(), vec![2], "{j:?}: live a microsecond before");
            assert!(locked(&c, 1, &super::super::FixedClock(before + chrono::Duration::microseconds(1))).unwrap().is_empty(), "{j:?}: not live at the deadline");
        }
        for (enabled, j) in [(Some(true), Some("FR")), (Some(false), Some("US")), (None, None)] {
            let c = db(enabled, j);
            assert_eq!(days(&c, 1).unwrap(), 0);
            assert_eq!(claim(&c, 1, 2, day("2026-09-20"), now("2026-09-20T12:00:00Z")).unwrap(), None);
            set(&c, Some("2027-01-01 00:00:00"), None);
            assert!(locked(&c, 1, &super::super::FixedClock(now("2026-09-20T12:00:00Z"))).unwrap().is_empty(), "rows stand, nothing is locked");
            assert!(!confirm_from_fill(&c, 1, 2, now("2026-09-20T12:00:00Z")).unwrap());
        }
    }

    #[test]
    fn claim_never_shortens_and_replaces_the_token() {
        let c = db(Some(true), None);
        set(&c, Some("2026-12-01 00:00:00"), None);
        let first = claim(&c, 1, 2, day("2026-09-20"), now("2026-09-20T12:00:00Z")).unwrap().unwrap();
        assert_eq!(first.previous.as_deref(), Some("2026-12-01 00:00:00"));
        assert_eq!(row(&c).0.as_deref(), Some("2026-12-01 00:00:00"));
        assert_eq!(row(&c).2.as_deref(), Some(first.token.as_str()));
        assert_eq!(first.token.len(), 16);
        let second = claim(&c, 1, 2, day("2026-09-20"), now("2026-09-20T12:00:00Z")).unwrap().unwrap();
        assert_ne!(first.token, second.token);
        assert_eq!(c.query_row("SELECT COUNT(*) FROM wash_sale_locks", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    }

    #[test]
    fn restore_is_floored_at_confirmed_and_ignores_a_foreign_token() {
        let c = db(Some(true), None);
        // No row before: restoring a fresh claim leaves no effective deadline.
        let fresh = claim(&c, 1, 2, day("2026-09-20"), now("2026-09-20T12:00:00Z")).unwrap().unwrap();
        restore(&c, 1, 2, &fresh).unwrap();
        assert_eq!((row(&c).0, row(&c).2), (None, None));
        // A fill confirmed meanwhile: the restore stops at it.
        let c = db(Some(true), None);
        set(&c, Some("2026-09-25 00:00:00"), None);
        let claimed = claim(&c, 1, 2, day("2026-09-20"), now("2026-09-20T12:00:00Z")).unwrap().unwrap();
        c.execute("UPDATE wash_sale_locks SET confirmed_locked_until = '2026-10-01 00:00:00'", []).unwrap();
        restore(&c, 1, 2, &claimed).unwrap();
        assert_eq!((row(&c).0.as_deref(), row(&c).2), (Some("2026-10-01 00:00:00"), None));
        // Previous above confirmed: back to previous.
        let c = db(Some(true), None);
        set(&c, Some("2026-10-05 00:00:00"), Some("2026-10-01 00:00:00"));
        let claimed = claim(&c, 1, 2, day("2026-09-20"), now("2026-09-20T12:00:00Z")).unwrap().unwrap();
        restore(&c, 1, 2, &claimed).unwrap();
        assert_eq!(row(&c).0.as_deref(), Some("2026-10-05 00:00:00"));
        // A newer claim took the row: the first claim's restore is a no-op.
        let first = claim(&c, 1, 2, day("2026-09-20"), now("2026-09-20T12:00:00Z")).unwrap().unwrap();
        let second = claim(&c, 1, 2, day("2026-09-20"), now("2026-09-20T12:00:00Z")).unwrap().unwrap();
        restore(&c, 1, 2, &first).unwrap();
        assert_eq!((row(&c).0.as_deref(), row(&c).2.as_deref()), (Some("2026-10-21 00:00:00"), Some(second.token.as_str())));
    }

    #[test]
    fn confirm_creates_or_raises_and_keeps_the_token() {
        let c = db(Some(true), None);
        assert!(confirm_from_fill(&c, 1, 2, now("2026-09-20T12:00:00Z")).unwrap(), "a missing row is created");
        assert_eq!(row(&c), (Some("2026-10-21 00:00:00".into()), Some("2026-10-21 00:00:00".into()), None, "bot".into()));
        let claimed = claim(&c, 1, 2, day("2026-09-25"), now("2026-09-25T12:00:00Z")).unwrap().unwrap();
        assert!(!confirm_from_fill(&c, 1, 2, now("2026-09-22T12:00:00Z")).unwrap(), "an earlier sale moves nothing");
        assert_eq!(row(&c).0.as_deref(), Some("2026-10-26 00:00:00"), "never shortened");
        assert_eq!(row(&c).1.as_deref(), Some("2026-10-23 00:00:00"), "confirmed is raised on its own");
        assert!(confirm_from_fill(&c, 1, 2, now("2026-09-30T12:00:00Z")).unwrap());
        assert_eq!(row(&c), (Some("2026-10-31 00:00:00".into()), Some("2026-10-31 00:00:00".into()), Some(claimed.token), "bot".into()));
    }

    #[test]
    fn log_locked_says_the_last_day_on_the_row() {
        let c = db(Some(true), None);
        set(&c, Some("2026-12-01 00:00:00"), None);
        let bot = model::load_bot(&c, 1).unwrap();
        log_locked(&c, &bot, 2, now("2026-09-20T12:00:00Z")).unwrap();
        let (event, level, details): (String, i64, String) = c.query_row("SELECT event, level, details FROM bot_activity_logs", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
        assert_eq!((event.as_str(), level, details.as_str()), ("wash_sale_locked", Level::Info as i64, r#"{"base":"AAA","until":"2026-11-30"}"#));
    }

    /// A row of the bot: (side, amount, quote, base, base_asset_id, status, external_status, amount_exec).
    type Row<'a> = (i64, &'a str, Option<&'a str>, &'a str, Option<i64>, i64, i64, Option<&'a str>);
    fn fills(c: &Connection, rows: &[Row<'_>]) {
        let before: usize = c.query_row("SELECT count(*) FROM transactions", [], |r| r.get(0)).unwrap();
        for (i, (side, amount, quote, base, asset, status, ext, exec)) in rows.iter().enumerate() {
            c.execute(&format!("INSERT INTO transactions(bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at)
                                VALUES (1,1,?1,?2,'USD',1,?3,?4,NULL,?5,?6,?7,?8,?9,'REGULAR',?10,{T})"),
                      params![base, asset, amount, exec, quote, side, status, ext, format!("o{}", before + i), format!("2026-03-0{} 00:00:00", before + i + 1)]).unwrap();
        }
    }

    fn at_loss(c: &Connection, amount: &str, quote: &str) -> bool {
        let bot = model::load_bot(c, 1).unwrap();
        let ticker = model::ticker_by_id(c, 1, 1).unwrap().unwrap();
        sell_at_loss(c, &bot, &ticker, &BigDec::parse(amount).unwrap(), &BigDec::parse(quote).unwrap(), now("2026-03-10T00:00:00Z")).unwrap()
    }

    #[test]
    fn sell_at_loss_verdicts() {
        // Off: never a loss, whatever the lots say.
        let c = db(Some(false), None);
        fills(&c, &[(0, "1", Some("100"), "AAA", Some(2), 0, 2, Some("1"))]);
        assert!(!at_loss(&c, "1", "50"));
        // No lots: nothing of its own to protect.
        let c = db(Some(true), None);
        assert!(!at_loss(&c, "1", "50"));
        // One lot of 3 bought for 100: a gain, a loss, and break-even cross-multiplied (not a loss).
        let c = db(Some(true), None);
        fills(&c, &[(0, "3", Some("100"), "AAA", Some(2), 0, 2, Some("3"))]);
        assert!(!at_loss(&c, "3", "150"));
        assert!(at_loss(&c, "3", "99"));
        assert!(!at_loss(&c, "3", "100"), "break-even");
        // Unknown cost reads as a loss (the walk records every bought lot's cost; the verdict still must not read
        // None as no loss).
        let d = |x: &str| Dec::parse(x).unwrap();
        let unknown: lots::Lots = [lots::Lot { amount: d("1"), cost: None }].into_iter().collect();
        assert!(judged(&unknown, &d("1"), &d("1000")).unwrap());
        assert!(!judged(&unknown, &d("0"), &d("1000")).unwrap(), "nothing sold is no loss");
        // A cheap lot then a dear one; this bot's resting sell of 1 takes the cheap one first.
        let c = db(Some(true), None);
        fills(&c, &[(0, "1", Some("10"), "AAA", Some(2), 0, 2, Some("1")), (0, "1", Some("100"), "AAA", Some(2), 0, 2, Some("1"))]);
        assert!(!at_loss(&c, "1", "50"), "the cheap lot is sold first");
        fills(&c, &[(1, "1", None, "AAA", None, 0, 0, None)]); // resting, recorded before orders stored their asset
        assert!(at_loss(&c, "1", "50"), "behind the resting sell, the dear lot");
        // A holding recorded without an asset under the ticker's name still holds units: a loss.
        let c = db(Some(true), None);
        fills(&c, &[(0, "1", Some("10"), "AAA", Some(2), 0, 2, Some("1")), (0, "1", Some("10"), "aaa", None, 0, 2, Some("1"))]);
        assert!(at_loss(&c, "1", "1000"));
    }
}
