//! AccountTransaction::SyncJob for a Bitget key (plan VB slice A4): Exchanges::Bitget#get_ledger (post-audit G6/G7, #608) on
//! the venue continuation (sync::continuation), then AccountTransactionSync#store! and TransferMatcher.run! as ledger.rs ports
//! them. Read only: signed GETs. Rows, tx_ids (a fill's `tradeId`, "deposit-"/"withdrawal-" and the record's `orderId`),
//! fees and raw_data are Rails' (rust/tests/sync_parity.rs, the `bitget-ledger-` scenarios).
//!
//! As Rails reads it:
//! - fills (/api/v2/spot/trade/fills): 89-day windows from the later of the start and 89 days before now (Bitget keeps 90
//!   days) up to now, each window paged newest first, 100 rows a page, the next page by `idLessThan` = the last row's
//!   `tradeId`, a short page ending the window;
//! - then, window by window from the start (2018-04-01 on a first sync), the deposit records and then the withdrawal records,
//!   paged the same way by the record's `orderId`; only a record whose status is `success` is an entry; 40014 or 40025 (a
//!   key without the wallet scope) ends that read as an empty one;
//! - any other error envelope, or failed read, fails the whole sync (G6: no partial success);
//! - a row read twice on the edge two windows share is one row (`entries.uniq`): a row with an id is met again by its id in
//!   the store, as one store meets it; a fill without a `tradeId` is compared whole (`Raw::eql_key`), across the import.
//!
//! Across runs (plan VB §1.6): the walk's cursor holds the source, its window's start, the next `idLessThan` and the whole
//! rows of the id-less fills it kept. A cut run replays its last page; the store's dedup state is the continuation's.
//!
//! Where it differs on purpose: an unreadable amount fails the sync (Rails stores a garbage amount as 0), as kucoin_ledger
//! does; a full page whose last row names no next `idLessThan`, or names the one just asked for, fails the sync
//! (D-BITGET-STUCK: Rails asks for the same page forever). AUDIT-bitget S2 (a BGB-deducted fee booked in `feeCoin`), S3 (a
//! withdrawal's `size` and `fee` booked apart) and S6 (`cTime` read in milliseconds) are ported as Rails reads them.
use super::activities::{Entry, Raw, BUY, DEPOSIT, SELL, WITHDRAWAL};
use super::binance_ledger::{amount, entry, objects, page_of, ruby_time_at_ms, text, Fail};
use super::continuation;
use super::ledger::Outcome;
use super::{capture_bound_version, load_key_of, phase, Failure, SyncError};
use crate::crypto::Credentials;
use crate::engine::Clock;
use crate::jobs::Db;
use crate::venue::bitget::{BitgetVenue, BITGET, OK, URL};
use crate::venue::http::Transport;
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};

pub const FILLS_PATH: &str = "/api/v2/spot/trade/fills";
pub const DEPOSITS_PATH: &str = "/api/v2/spot/wallet/deposit-records";
pub const WITHDRAWALS_PATH: &str = "/api/v2/spot/wallet/withdrawal-records";
/// Exchanges::Bitget::LEDGER_PAGE: every read's limit and its largest page.
pub const PAGE: usize = 100;
/// Exchanges::Bitget::LEDGER_WINDOW and FILLS_REACH: 89 days.
pub const WINDOW_DAYS: i64 = 89;
/// Exchanges::Bitget::LEDGER_NOT_PERMITTED: a transfer read refused for the wallet scope reads as empty.
pub const NOT_PERMITTED: [&str; 2] = ["40014", "40025"];
/// Exchanges::Bitget::TRANSFERS_FROM: Bitget opened in 2018.
pub fn transfers_from() -> DateTime<Utc> { "2018-04-01T00:00:00Z".parse().expect("a fixed time") }
pub const OVERLAP_HOURS: i64 = 25;
pub const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PAGE_NODES: usize = 400_000;
/// ponytail: Rails has no page cap; one stops a cursor that never ends.
const MAX_PAGES: usize = 100_000;

/// `#to_i` of a JSON value (nil.to_i is 0).
fn to_i(v: &Value) -> i64 { match v { Value::Number(n) => n.as_i64().unwrap_or_else(|| n.as_f64().map_or(0, |f| f as i64)), Value::String(s) => crate::ruby::to_i(s), _ => 0 } }
/// `#to_s` of a scalar, as Ruby interpolates it (nil is "").
fn to_s(v: &Value) -> String { match v { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() } }
fn ms(t: DateTime<Utc>) -> i64 { crate::venue::bitvavo::ruby_ms(t) }
/// BigDecimal#abs.
fn abs(d: crate::ruby::BigDec) -> crate::ruby::BigDec { if d < crate::ruby::BigDec::zero() { &crate::ruby::BigDec::zero() - &d } else { d } }

/// The exchange's tickers as #symbol_pair_base / #symbol_pair_quote find them (`tickers.find_by(ticker:)`: the first).
struct Tickers(Vec<(String, String, String)>);
impl Tickers {
    fn load(c: &rusqlite::Connection, exchange_id: i64) -> Result<Self, SyncError> {
        let mut s = c.prepare("SELECT ticker, base, quote FROM tickers WHERE exchange_id = ?1 ORDER BY id")?;
        let rows = s.query_map([exchange_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(Self(rows))
    }
    /// [base, quote] of the listed pair, else the symbol for both.
    fn pair(&self, symbol: Option<&str>) -> (Option<String>, Option<String>) {
        match self.0.iter().find(|(t, _, _)| Some(t.as_str()) == symbol) {
            Some((_, base, quote)) => (Some(base.clone()), Some(quote.clone())),
            None => (symbol.map(str::to_string), symbol.map(str::to_string)),
        }
    }
}

/// One fill as #get_ledger maps it: a buy or a sell of the pair's base, quoted at size × priceAvg, the fee `feeDetail`'s
/// totalFee in its feeCoin (else `fee`, in the quote), keyed by `tradeId`.
fn fill(raw: Raw, tickers: &Tickers) -> Result<Entry, Fail> {
    let (base, quote) = tickers.pair(raw.value["symbol"].as_str());
    let side = match &raw.value["side"] { Value::Null => None, Value::String(s) => Some(s.to_lowercase()), _ => return Err(("unreadable fill side".into(), true)) };
    let at = ruby_time_at_ms(to_i(&raw.value["cTime"])).ok_or_else(|| ("unreadable fill cTime".to_string(), true))?;
    let size = amount(&raw, "size", "fill")?;
    let price = raw.decimal("priceAvg").map_err(|why| (format!("fill: {why}"), true))?;
    let detail = raw.object("feeDetail").map_err(|why| (format!("fill: {why}"), true))?;
    let total = match &detail { Some(d) => d.number("totalFee").map_err(|why| (format!("fill: {why}"), true))?, None => None };
    let fee = abs(match total { Some(t) => t, None => raw.decimal("fee").map_err(|why| (format!("fill: {why}"), true))? });
    let mut e = entry(if side.as_deref() == Some("buy") { BUY } else { SELL }, "", size.clone(), &raw, at);
    e.base_currency = base;
    e.quote_amount = Some(&size * &price);
    e.fee_currency = detail.as_ref().and_then(|d| d.value.get("feeCoin")).filter(|v| !v.is_null()).map(to_s).or_else(|| quote.clone());
    e.quote_currency = quote;
    e.fee_amount = Some(fee);
    e.tx_id = text(raw.value.get("tradeId"));
    Ok(e)
}

/// One deposit or withdrawal record: an entry only at status `success`, keyed by its orderId; a withdrawal's fee is `fee.abs`
/// in its coin (a zero one is stored as none).
fn transfer(kind: i64, raw: Raw) -> Result<Option<Entry>, Fail> {
    if raw.value["status"].as_str() != Some("success") { return Ok(None); }
    let at = ruby_time_at_ms(to_i(&raw.value["cTime"])).ok_or_else(|| ("unreadable transfer cTime".to_string(), true))?;
    let coin = raw.value.get("coin").filter(|v| !v.is_null()).map(to_s);
    let what = if kind == DEPOSIT { "deposit" } else { "withdrawal" };
    let mut e = entry(kind, "", amount(&raw, "size", what)?, &raw, at);
    e.base_currency = coin.clone();
    if kind == WITHDRAWAL {
        e.fee_currency = coin;
        e.fee_amount = Some(abs(raw.decimal("fee").map_err(|why| (format!("withdrawal: {why}"), true))?));
    }
    e.tx_id = Some(format!("{what}-{}", to_s(&raw.value["orderId"])));
    Ok(Some(e))
}

/// Which read of Exchanges::Bitget#get_ledger the walk is in.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Source { Fills, Deposits, Withdrawals, Done }

impl Source {
    fn name(self) -> &'static str { match self { Self::Fills => "fills", Self::Deposits => "deposits", Self::Withdrawals => "withdrawals", Self::Done => "done" } }
    fn named(s: &str) -> Option<Self> { [Self::Fills, Self::Deposits, Self::Withdrawals, Self::Done].into_iter().find(|v| v.name() == s) }
    fn path(self) -> &'static str { match self { Self::Fills => FILLS_PATH, Self::Deposits => DEPOSITS_PATH, _ => WITHDRAWALS_PATH } }
    fn id(self) -> &'static str { if self == Self::Fills { "tradeId" } else { "orderId" } }
}

/// Exchanges::Bitget#get_ledger from `start` (the watermark less the overlap; None is a first sync) to `now_ms` (the import's
/// first run), a page at a time: `window` is the start of the window being read (its end 89 days on, or now), `before` the
/// next page's `idLessThan`, `bare` the id-less fills kept so far (#get_ledger's `uniq` for rows no id names).
struct Walk<'a, T: Transport> {
    venue: &'a BitgetVenue<T>, tickers: &'a Tickers, now_ms: i64, transfers_ms: i64,
    source: Source, window: i64, before: Option<String>, bare: Vec<String>, pages: usize,
}

impl<'a, T: Transport> Walk<'a, T> {
    fn new(venue: &'a BitgetVenue<T>, tickers: &'a Tickers, begin: continuation::Begin<'_>) -> Self {
        let now_ms = ms(begin.end);
        // `[start_time, FILLS_REACH.ago].compact.max`, and `start_time || TRANSFERS_FROM`.
        let reach = begin.end - Duration::days(WINDOW_DAYS);
        let fills_ms = ms(begin.start.map_or(reach, |s| s.max(reach)));
        let transfers_ms = ms(begin.start.unwrap_or_else(transfers_from));
        let mut walk = Self { venue, tickers, now_ms, transfers_ms, source: Source::Fills, window: fills_ms, before: None, bare: vec![], pages: 0 };
        if let Some((c, source)) = begin.cursor.and_then(|c| Some((c, Source::named(c["source"].as_str()?)?))) {
            walk.source = source;
            walk.window = c["window"].as_i64().unwrap_or(fills_ms);
            walk.before = c["before"].as_str().map(str::to_string);
            walk.bare = c["bare"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect();
        }
        walk.settle();
        walk
    }

    fn window_end(&self) -> i64 { (self.window + WINDOW_DAYS * 86_400_000).min(self.now_ms) }

    /// Past the last window of a read: the fills hand over to the first transfer window; the transfers end.
    fn settle(&mut self) {
        if self.source == Source::Fills && self.window >= self.now_ms { (self.source, self.window) = (Source::Deposits, self.transfers_ms); }
        if matches!(self.source, Source::Deposits | Source::Withdrawals) && self.window >= self.now_ms { self.source = Source::Done; }
    }

    /// The read the walk is on ended (a short page, or a refused wallet scope): the next one.
    fn next_read(&mut self) {
        self.before = None;
        match self.source {
            Source::Fills => self.window = self.window_end(),
            Source::Deposits => self.source = Source::Withdrawals,
            Source::Withdrawals => (self.source, self.window) = (Source::Deposits, self.window_end()),
            Source::Done => {}
        }
        self.settle();
    }
}

impl<T: Transport> continuation::Walk for Walk<'_, T> {
    fn done(&self) -> bool { self.source == Source::Done }
    fn cursor(&self) -> Value { json!({ "source": self.source.name(), "window": self.window, "before": self.before, "bare": self.bare }) }
    fn page_rows(&self) -> usize { PAGE }
    /// One try at the transport's whole budget: Rails paces none of these reads.
    fn request_reserve(&self) -> std::time::Duration { crate::venue::http::TOTAL_TIMEOUT }
    /// One page of #each_ledger_row.
    async fn step(&mut self) -> Result<Vec<Entry>, Fail> {
        if self.source == Source::Done { return Ok(vec![]); }
        if self.pages >= MAX_PAGES { return Err(("Too many pages of Bitget history".into(), false)); }
        self.pages += 1;
        let mut query = vec![("startTime", self.window.to_string()), ("endTime", self.window_end().to_string()), ("limit", PAGE.to_string())];
        query.extend(self.before.clone().map(|b| ("idLessThan", b)));
        let optional = self.source != Source::Fills;
        let text = match self.venue.read_text(self.source.path(), query, MAX_PAGE_BYTES).await {
            Ok(text) => text,
            // #bitget_envelope of a failure: its JSON body's code; a refused wallet scope reads as empty.
            Err(text) => {
                let code = crate::venue::bitget::envelope(&text).0;
                if optional && code.as_ref().and_then(Value::as_str).is_some_and(|c| NOT_PERMITTED.contains(&c)) { self.next_read(); return Ok(vec![]); }
                return Err((text, false));
            }
        };
        // Faraday leaves an empty 2xx nil, and a 2xx that is no Hash carries no code: "Bitget ledger read failed: ".
        let unreadable_envelope = || ("Bitget ledger read failed: ".to_string(), false);
        if text.trim().is_empty() { return Err(unreadable_envelope()); }
        let Ok(body) = Raw::from_node(&page_of(&text, "Bitget ledger", MAX_PAGE_NODES)?) else { return Err(unreadable_envelope()) };
        let code = body.value.get("code").filter(|v| !v.is_null());
        if optional && code.and_then(Value::as_str).is_some_and(|c| NOT_PERMITTED.contains(&c)) { self.next_read(); return Ok(vec![]); }
        if code.and_then(Value::as_str) != Some(OK) {
            let parts: Vec<String> = [code, body.value.get("msg").filter(|v| !v.is_null())].into_iter().flatten().map(to_s).collect();
            return Err((format!("Bitget ledger read failed: {}", parts.join(" ")), false));
        }
        // `Array(result.data['data'])`: none is an empty page; anything but a list of objects Rails cannot walk.
        let page = match body.member("data") {
            None => vec![],
            Some(super::wire::Node::Scalar(t)) if t == "null" => vec![],
            Some(node) => objects(node, "Bitget ledger row")?,
        };
        let mut rows = vec![];
        for raw in &page {
            match self.source {
                Source::Fills => {
                    let e = fill(raw.clone(), self.tickers)?;
                    if e.tx_id.as_deref().is_none_or(crate::ruby::blank) {
                        let key = raw.eql_key();
                        if self.bare.contains(&key) { continue; }
                        self.bare.push(key);
                    }
                    rows.push(e);
                }
                kind => if let Some(e) = transfer(if kind == Source::Deposits { DEPOSIT } else { WITHDRAWAL }, raw.clone())? { rows.push(e) },
            }
        }
        if page.len() < PAGE { self.next_read(); return Ok(rows); }
        let next = page.last().and_then(|r| r.value.get(self.source.id())).filter(|v| !v.is_null()).map(to_s);
        if next.is_none() || next == self.before {
            return Err((format!("Bitget {} paging stuck at idLessThan {}", self.source.name(), self.before.clone().unwrap_or_default()), false));
        }
        self.before = next;
        Ok(rows)
    }
}

/// One ledger sync of one Bitget key, as AccountTransaction::SyncJob#perform runs it, held to the job's deadline
/// (sync::continuation): a long import goes on over several runs. Idempotent.
pub async fn sync<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock) -> Result<Result<Outcome, Failure>, SyncError> {
    let stopwatch = continuation::Wall::start();
    let run = continuation::Run { deadline: super::jobs::LEDGER_DEADLINE, chain_runs: continuation::CHAIN_RUNS, stopwatch: &stopwatch };
    sync_within(db, transport, key_id, credentials, clock, &run).await
}

/// `sync` within `run`'s bounds (a test states a short deadline, a scripted stopwatch or a short chain).
pub async fn sync_within<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock, run: &continuation::Run<'_>)
                                       -> Result<Result<Outcome, Failure>, SyncError> {
    let version = capture_bound_version(db, key_id, credentials).await?;
    let now = clock.now();
    let captured = version.clone();
    let (key, produced, tickers) = phase(db, move |c| {
        let key = load_key_of(c, key_id, &[BITGET])?;
        // A watermark another credential produced is read as none: the history is read whole (#511).
        let produced = super::cache::ledger_produced_by(c, key_id, &captured)?;
        let tickers = Tickers::load(c, key.exchange_id)?;
        Ok((key, produced, tickers))
    }).await?;
    let watermark = if produced { key.last_synced_at } else { None };
    let venue = BitgetVenue::new(transport, URL);
    continuation::sync(db, &version, key, watermark, Duration::hours(OVERLAP_HOURS), credentials, now, run, |begin| Walk::new(&venue, &tickers, begin)).await
}

#[cfg(test)]
mod tests {
    use super::Tickers;

    #[test]
    fn a_fill_names_its_listed_pair_else_its_symbol_twice() {
        let t = Tickers(vec![("BTCUSDT".into(), "BTC".into(), "USDT".into()), ("BTCUSDT".into(), "XBT".into(), "USDT".into())]);
        assert_eq!(t.pair(Some("BTCUSDT")), (Some("BTC".into()), Some("USDT".into())), "the first ticker");
        assert_eq!(t.pair(Some("NEWUSDT")), (Some("NEWUSDT".into()), Some("NEWUSDT".into())));
        assert_eq!(t.pair(None), (None, None));
    }
}
