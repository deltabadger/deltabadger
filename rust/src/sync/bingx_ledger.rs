//! BingX B6–B8 on the shared continuation kit. Transfers precede pair discovery; S2's seven-day
//! trade reach is preserved. D-BINGX-PAGE names the failed endpoint instead of Rails' undefined `path`.
use super::activities::{Entry, Raw, BUY, DEPOSIT, SELL, WITHDRAWAL};
use super::binance_ledger::{amount, entry, page_of, ruby_time_at_ms, text, Fail};
use super::bitrue_ledger::listing_field as index; // Ruby's `value['key']`
use super::ledger::Outcome;
use super::wire::Node;
use super::{capture_bound_version, continuation, load_key_of, phase, Failure, Key, SyncError};
use crate::crypto::Credentials;
use crate::engine::Clock;
use crate::jobs::Db;
use crate::venue::bingx::{BingxVenue, ACCOUNT_PATH, BINGX, URL};
use crate::venue::bitvavo::ruby_ms;
use crate::venue::http::Transport;
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashSet};

pub const TRADES_PATH: &str = "/openApi/spot/v1/trade/myTrades";
pub const DEPOSITS_PATH: &str = "/openApi/api/v3/capital/deposit/hisrec";
pub const WITHDRAWALS_PATH: &str = "/openApi/api/v3/capital/withdraw/history";
pub const SYMBOLS_PATH: &str = "/openApi/spot/v1/common/symbols";
const TRANSFER_PAGE: usize = 1000;
const TRADE_PAGE: usize = 500;
const WINDOW: i64 = 90 * 86_400_000;
const REACH: i64 = 7 * 86_400_000 - 300_000;
fn to_i(v: &Value) -> i64 {
    match v {
        Value::String(s) => crate::ruby::to_i(s),
        Value::Number(n) => n
            .as_i64()
            .unwrap_or_else(|| n.as_f64().unwrap_or_default() as i64),
        _ => 0,
    }
}
fn field(v: &Value, k: &str) -> String {
    text(v.get(k)).unwrap_or_default()
}
fn raw(s: &str) -> Result<Raw, Fail> {
    Raw::parse(s)
        .and_then(Result::ok)
        .ok_or_else(|| ("unreadable BingX ledger row".into(), true))
}
fn member<'a>(n: &'a Node, key: &str) -> Option<&'a Node> {
    if let Node::Object(m) = n {
        m.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    } else {
        None
    }
}
fn rows(n: Option<&Node>) -> Result<Vec<String>, Fail> {
    match n {
        Some(Node::Array(a)) => Ok(a.iter().map(Node::text).collect()),
        None => Ok(vec![]),
        Some(Node::Scalar(s)) if s == "null" => Ok(vec![]),
        _ => Err(("BingX ledger read failed: unreadable rows".into(), false)),
    }
}
/// Ruby's `hash.dig('data', key)` on an answer whose `['code']` already read: a nested non-Hash raises as Ruby does.
fn dig(v: &Value, key: &str) -> Result<Value, Fail> {
    let data = match v {
        Value::Object(m) => m.get("data").cloned().unwrap_or(Value::Null),
        other => return Err((format!("NoMethodError: undefined method 'dig' for {}", crate::web::launchpad::receiver(other)), true)),
    };
    let class = match &data {
        Value::Null => return Ok(Value::Null),
        Value::Object(m) => return Ok(m.get(key).cloned().unwrap_or(Value::Null)),
        Value::Array(_) => return Err(("TypeError: no implicit conversion of String into Integer".into(), true)),
        Value::String(_) => "String",
        Value::Number(n) if n.is_f64() => "Float",
        Value::Number(_) => "Integer",
        Value::Bool(true) => "TrueClass",
        Value::Bool(false) => "FalseClass",
    };
    Err((format!("TypeError: {class} does not have #dig method"), true))
}
/// Ruby's `Array(v)`.
fn array(v: Value) -> Vec<Value> {
    match v {
        Value::Null => vec![],
        Value::Array(a) => a,
        Value::Object(m) => m.into_iter().map(|(k, v)| json!([k, v])).collect(),
        scalar => vec![scalar],
    }
}
/// Ruby's `v.to_d.positive?`.
fn positive(v: &Value) -> Result<bool, Fail> {
    match v {
        Value::Bool(_) | Value::Array(_) | Value::Object(_) => Err((format!("NoMethodError: undefined method 'to_d' for {}", crate::web::launchpad::receiver(v)), true)),
        v => Ok(crate::ruby::json_to_d(v).ok().flatten().is_some_and(|n| n.is_positive())),
    }
}
fn value(n: &Node) -> Result<Value, Fail> {
    serde_json::from_str(&n.text()).map_err(|_| ("unreadable BingX response".into(), true))
}
fn envelope(v: &Value) -> Result<(), Fail> {
    if to_i(&v["code"]) != 0 {
        Err((
            format!(
                "BingX ledger read failed: {} {}",
                field(v, "code"),
                field(v, "msg")
            ),
            false,
        ))
    } else {
        Ok(())
    }
}
fn transfer(r: Raw, deposit: bool) -> Result<Entry, Fail> {
    let coin = field(&r.value, "coin");
    let network = field(&r.value, "network");
    let coin = if !network.is_empty() && coin != network {
        coin.strip_suffix(&network).unwrap_or(&coin).to_string()
    } else {
        coin
    };
    let at = if deposit {
        ruby_time_at_ms(to_i(&r.value["insertTime"]))
    } else {
        let time = field(&r.value, "applyTime");
        if time.bytes().all(|b| b.is_ascii_digit()) {
            ruby_time_at_ms(crate::ruby::to_i(&time))
        } else {
            chrono::DateTime::parse_from_rfc3339(&time)
                .map(|t| t.with_timezone(&Utc))
                .ok()
                .or_else(|| {
                    chrono::NaiveDateTime::parse_from_str(&time, "%Y-%m-%d %H:%M:%S")
                        .ok()
                        .map(|t| t.and_utc())
                })
        }
    }
    .ok_or_else(|| ("unreadable transfer time".into(), true))?;
    let mut e = entry(
        if deposit { DEPOSIT } else { WITHDRAWAL },
        &coin,
        amount(&r, "amount", "transfer")?,
        &r,
        at,
    );
    e.tx_id =
        text(r.value.get(if deposit { "txId" } else { "id" })).filter(|s| !s.trim().is_empty());
    if !deposit {
        e.fee_currency = Some(coin);
        e.fee_amount = r.number("transactionFee").map_err(|e| (e, true))?;
    }
    Ok(e)
}
fn trade(symbol: &str, r: Raw) -> Result<Entry, Fail> {
    let (base, quote) = symbol.split_once('-').unwrap_or((symbol, ""));
    let at = ruby_time_at_ms(to_i(&r.value["time"]))
        .ok_or_else(|| ("unreadable trade time".into(), true))?;
    let buy = !matches!(
        r.value.get("isBuyer"),
        None | Some(Value::Null | Value::Bool(false))
    );
    let mut e = entry(
        if buy { BUY } else { SELL },
        base,
        amount(&r, "qty", "trade")?,
        &r,
        at,
    );
    e.quote_currency = Some(quote.into());
    e.quote_amount = Some(amount(&r, "quoteQty", "trade")?);
    e.fee_currency = text(r.value.get("commissionAsset"));
    let fee = amount(&r, "commission", "trade")?;
    e.fee_amount = Some(if fee < crate::ruby::BigDec::zero() {
        &crate::ruby::BigDec::zero() - &fee
    } else {
        fee
    });
    e.tx_id = Some(field(&r.value, "id"));
    Ok(e)
}
#[derive(Clone, Copy, PartialEq)]
enum Source {
    Deposits,
    Withdrawals,
    Account,
    Listing,
    Trades,
    Done,
}
struct Cursor {
    source: Source,
    window: i64,
    offset: usize,
    seen: HashSet<Value>,
    discovered: BTreeSet<String>,
    pairs: Vec<String>,
    pair: usize,
    ranges: Vec<(i64, i64)>,
    pending: Vec<String>,
}
impl Source {
    fn label(self) -> &'static str {
        match self {
            Self::Deposits => "deposits",
            Self::Withdrawals => "withdrawals",
            Self::Account => "account",
            Self::Listing => "listing",
            Self::Trades => "trades",
            Self::Done => "done",
        }
    }
    fn of(s: &str) -> Option<Self> {
        Some(match s {
            "deposits" => Self::Deposits,
            "withdrawals" => Self::Withdrawals,
            "account" => Self::Account,
            "listing" => Self::Listing,
            "trades" => Self::Trades,
            "done" => Self::Done,
            _ => return None,
        })
    }
}
impl Cursor {
    fn json(&self) -> Value {
        json!({"source":self.source.label(),"window":self.window,"offset":self.offset,"seen":self.seen,
        "discovered":self.discovered,"pairs":self.pairs,"pair":self.pair,"ranges":self.ranges,"pending":self.pending})
    }
    fn of(v: &Value) -> Option<Self> {
        Some(Self {
            source: Source::of(v["source"].as_str()?)?,
            window: v["window"].as_i64()?,
            offset: v["offset"].as_u64()?.try_into().ok()?,
            seen: serde_json::from_value(v["seen"].clone()).ok()?,
            discovered: serde_json::from_value(v["discovered"].clone()).ok()?,
            pairs: serde_json::from_value(v["pairs"].clone()).ok()?,
            pair: v["pair"].as_u64()?.try_into().ok()?,
            ranges: serde_json::from_value(v["ranges"].clone()).ok()?,
            pending: serde_json::from_value(v["pending"].clone()).ok()?,
        })
    }
}
struct Walk<'a, T: Transport> {
    venue: &'a BingxVenue<T>,
    db: &'a Db,
    key: &'a Key,
    clock: &'a dyn Clock,
    start: i64,
    end: i64,
    c: Cursor,
}
impl<'a, T: Transport> Walk<'a, T> {
    fn new(
        venue: &'a BingxVenue<T>,
        db: &'a Db,
        key: &'a Key,
        clock: &'a dyn Clock,
        begin: continuation::Begin<'_>,
    ) -> Self {
        let start = ruby_ms(
            begin
                .start
                .unwrap_or_else(|| "2018-01-01T00:00:00Z".parse().expect("BingX opening")),
        );
        let c = begin.cursor.and_then(Cursor::of).unwrap_or(Cursor {
            source: Source::Deposits,
            window: start,
            offset: 0,
            seen: HashSet::new(),
            discovered: BTreeSet::new(),
            pairs: vec![],
            pair: 0,
            ranges: vec![],
            pending: vec![],
        });
        Self {
            venue,
            db,
            key,
            clock,
            start,
            end: ruby_ms(begin.end),
            c,
        }
    }
    async fn get(&self, path: &str, q: Vec<(&'static str, String)>) -> Result<Node, Fail> {
        let r = if path == SYMBOLS_PATH {
            self.venue.request("GET", path, q)
        } else {
            self.venue.signed("GET", path, q, self.clock.now())
        };
        let answer = self
            .venue
            .read_text(&r, 64 * 1024 * 1024)
            .await
            .map_err(|s| (s, false))?;
        page_of(&answer, "BingX ledger", 8_000_000)
    }
    fn next_transfer(&mut self) {
        self.c.source = if self.c.source == Source::Deposits {
            Source::Withdrawals
        } else {
            Source::Account
        };
        self.c.window = self.start;
        self.c.offset = 0;
        self.c.seen.clear();
    }
    async fn transfers(&mut self) -> Result<Vec<Entry>, Fail> {
        if self.c.window >= self.end {
            self.next_transfer();
            return Ok(vec![]);
        }
        let deposit = self.c.source == Source::Deposits;
        let path = if deposit {
            DEPOSITS_PATH
        } else {
            WITHDRAWALS_PATH
        };
        let end = (self.c.window + WINDOW).min(self.end);
        let n = self
            .get(
                path,
                vec![
                    ("startTime", self.c.window.to_string()),
                    ("endTime", end.to_string()),
                    ("offset", self.c.offset.to_string()),
                    ("limit", TRANSFER_PAGE.to_string()),
                ],
            )
            .await?;
        if let Node::Object(_) = n {
            let v = value(&n)?;
            if to_i(&v["code"]) == 100004 {
                self.next_transfer();
                return Ok(vec![]);
            }
            envelope(&v)?;
        }
        let page = rows(if matches!(n, Node::Array(_)) {
            Some(&n)
        } else {
            member(&n, "data")
        })?;
        let count = page.len();
        let mut entries = vec![];
        let mut fresh = 0;
        for s in page {
            let r = raw(&s)?;
            if !self.c.seen.insert(r.value.clone()) {
                continue;
            }
            fresh += 1;
            if to_i(&r.value["status"]) != if deposit { 1 } else { 6 } {
                continue;
            }
            let e = transfer(r, deposit)?;
            self.c.discovered.extend(e.base_currency.clone());
            entries.push(e);
        }
        if count >= TRANSFER_PAGE {
            if fresh == 0 {
                return Err((
                    format!("BingX ledger read failed: a full page of {path} brought nothing new"),
                    false,
                ));
            }
            self.c.offset += count;
        } else {
            self.c.window = end;
            self.c.offset = 0;
        }
        if self.c.window >= self.end {
            self.next_transfer()
        }
        Ok(entries)
    }
    async fn account(&mut self) -> Result<Vec<Entry>, Fail> {
        let v = value(&self.get(ACCOUNT_PATH, vec![]).await?)?;
        if to_i(&index(&v, "code")?) != 0 {
            return Err((
                format!("BingX ledger read failed: {}", field(&v, "msg")),
                false,
            ));
        }
        // As Rails reads it: an answer of another shape raises before any pair is skipped.
        for bal in array(dig(&v, "balances")?) {
            if positive(&index(&bal, "free")?)? || positive(&index(&bal, "locked")?)? {
                if let Value::String(asset) = index(&bal, "asset")? {
                    self.c.discovered.insert(asset);
                }
            }
        }
        self.c.source = Source::Listing;
        Ok(vec![])
    }
    async fn listing(&mut self) -> Result<Vec<Entry>, Fail> {
        let v = value(&self.get(SYMBOLS_PATH, vec![]).await?)?;
        if to_i(&index(&v, "code")?) != 0 {
            return Err((
                format!("BingX ledger read failed: {}", field(&v, "msg")),
                false,
            ));
        }
        let symbols = array(dig(&v, "symbols")?).iter().map(|s| index(s, "symbol")).collect::<Result<Vec<_>, _>>()?;
        let (user, exchange) = (self.key.user_id, self.key.exchange_id);
        let before:HashSet<String>=phase(self.db,move |db| {
            let mut s=db.prepare("SELECT base_currency, quote_currency FROM account_transactions WHERE user_id=?1 AND exchange_id=?2 AND entry_type IN (0,1) UNION SELECT t.base,t.quote FROM transactions t JOIN bots b ON b.id=t.bot_id WHERE b.user_id=?1 AND t.exchange_id=?2")?;
            let rows=s.query_map([user,exchange],|r|Ok(format!("{}-{}",r.get::<_,Option<String>>(0)?.unwrap_or_default(),r.get::<_,Option<String>>(1)?.unwrap_or_default())))?;
            Ok(rows.collect::<Result<HashSet<_>,_>>()?)
        }).await.map_err(|e|(e.0,true))?;
        for symbol in symbols {
            let Value::String(symbol) = symbol else {
                return Err((format!("NoMethodError: undefined method 'split' for {}", crate::web::launchpad::receiver(&symbol)), true));
            };
            if self
                .c
                .discovered
                .contains(symbol.split('-').next().unwrap_or_default())
                || before.contains(&symbol)
            {
                self.c.pairs.push(symbol)
            }
        }
        self.c.source = if self.c.pairs.is_empty() {
            Source::Done
        } else {
            Source::Trades
        };
        Ok(vec![])
    }
    async fn trades(&mut self) -> Result<Vec<Entry>, Fail> {
        let symbol = self.c.pairs[self.c.pair].clone();
        // myTrades serves seven days back from the moment it is asked: the reach is taken now, not when the
        // import began, and a range kept by an earlier run is clamped to it before it is sent.
        let reach = self.start.max(ruby_ms(self.clock.now()) - REACH);
        if self.c.ranges.is_empty() {
            self.c.ranges.push((reach, self.end));
        }
        let (from, to) = self.c.ranges.pop().expect("range");
        let from = from.max(reach);
        // A range wholly past the seven days has nothing left to read.
        let page = if from >= to {
            vec![]
        } else {
            let n = self
                .get(
                    TRADES_PATH,
                    vec![
                        ("symbol", symbol.clone()),
                        ("startTime", from.to_string()),
                        ("endTime", to.to_string()),
                        ("limit", TRADE_PAGE.to_string()),
                    ],
                )
                .await?;
            let v = value(&n)?;
            if [100204, 100490].contains(&to_i(&v["code"])) {
                vec![]
            } else {
                envelope(&v)?;
                rows(member(&n, "data").and_then(|n| member(n, "fills")))?
            }
        };
        if page.len() >= TRADE_PAGE {
            if to - from < 2 {
                return Err((format!("BingX ledger read failed: over {TRADE_PAGE} {symbol} trades in {from}..{to}"),false));
            }
            let mid = (from + to) / 2;
            self.c.ranges.extend([(mid, to), (from, mid)]);
        } else {
            self.c.pending.extend(page);
        }
        if !self.c.ranges.is_empty() {
            return Ok(vec![]);
        }
        let mut ids = HashSet::new();
        let mut entries = vec![];
        for s in std::mem::take(&mut self.c.pending) {
            let r = raw(&s)?;
            if ids.insert(r.value["id"].clone()) {
                entries.push(trade(&symbol, r)?)
            }
        }
        self.c.pair += 1;
        if self.c.pair == self.c.pairs.len() {
            self.c.source = Source::Done
        }
        Ok(entries)
    }
}
impl<T: Transport> continuation::Walk for Walk<'_, T> {
    fn done(&self) -> bool {
        self.c.source == Source::Done
    }
    fn cursor(&self) -> Value {
        self.c.json()
    }
    fn page_rows(&self) -> usize {
        TRANSFER_PAGE + self.c.pending.len()
    }
    fn request_reserve(&self) -> std::time::Duration {
        crate::venue::http::TOTAL_TIMEOUT
    }
    async fn step(&mut self) -> Result<Vec<Entry>, Fail> {
        match self.c.source {
            Source::Deposits | Source::Withdrawals => self.transfers().await,
            Source::Account => self.account().await,
            Source::Listing => self.listing().await,
            Source::Trades => self.trades().await,
            Source::Done => Ok(vec![]),
        }
    }
}
pub async fn sync<T: Transport>(
    db: &Db,
    transport: T,
    key_id: i64,
    credentials: &Credentials,
    clock: &dyn Clock,
) -> Result<Result<Outcome, Failure>, SyncError> {
    let watch = continuation::Wall::start();
    let run = continuation::Run {
        deadline: super::jobs::LEDGER_DEADLINE,
        chain_runs: continuation::CHAIN_RUNS,
        stopwatch: &watch,
    };
    sync_within(db, transport, key_id, credentials, clock, &run).await
}
pub async fn sync_within<T: Transport>(
    db: &Db,
    transport: T,
    key_id: i64,
    credentials: &Credentials,
    clock: &dyn Clock,
    run: &continuation::Run<'_>,
) -> Result<Result<Outcome, Failure>, SyncError> {
    let version = capture_bound_version(db, key_id, credentials).await?;
    let captured = version.clone();
    let (key, produced) = phase(db, move |c| {
        Ok((
            load_key_of(c, key_id, &[BINGX])?,
            super::cache::ledger_produced_by(c, key_id, &captured)?,
        ))
    })
    .await?;
    let watermark = if produced { key.last_synced_at } else { None };
    let venue = BingxVenue::new(transport, URL, &credentials.secret);
    let walking = key.clone();
    continuation::sync(
        db,
        &version,
        key,
        watermark,
        Duration::hours(25),
        credentials,
        clock.now(),
        run,
        |begin| Walk::new(&venue, db, &walking, clock, begin),
    )
    .await
}
