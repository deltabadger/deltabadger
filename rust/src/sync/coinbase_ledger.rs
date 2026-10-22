//! Coinbase fills, every account, then each account's transfers on the continuation kit.
//! D-CB-SIZE-IN-QUOTE: quote-sized fills book size / price base and size quote; commission stays separate.
use super::activities::{Entry, Raw, BUY, DEPOSIT, SELL, SWAP_IN, SWAP_OUT, WITHDRAWAL};
use super::binance_ledger::{entry, objects, page_of, text, Fail};
use super::ledger::Outcome;
use super::{capture_bound_version, continuation, load_key_of, phase, Failure, SyncError};
use crate::crypto::Credentials;
use crate::engine::Clock;
use crate::jobs::Db;
use crate::ruby::BigDec;
use crate::venue::coinbase::{CoinbaseVenue, COINBASE, URL};
use crate::venue::http::Transport;
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
pub const FILLS: &str = "/api/v3/brokerage/orders/historical/fills";
pub const ACCOUNTS: &str = "/api/v3/brokerage/accounts";
pub const FIAT: [&str; 7] = ["USD", "EUR", "GBP", "CAD", "AUD", "JPY", "CHF"];
fn fail(m: impl Into<String>) -> Fail {
    (m.into(), true)
}
fn time(v: &Value) -> Result<DateTime<Utc>, Fail> {
    v.as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| fail("unreadable Coinbase time"))
}
/// Shared by imports and the one-time stored-row repair. A zero price cannot describe a quote-sized fill.
pub fn amounts(raw: &Raw) -> Result<(BigDec, BigDec), Fail> {
    let size = raw.decimal("size").map_err(fail)?;
    let price = raw.decimal("price").map_err(fail)?;
    if raw.value["size_in_quote"] == true {
        if !price.is_positive() {
            return Err(fail("Coinbase quote-sized fill requires a positive price"));
        }
        Ok((
            size.div(&price)
                .ok_or_else(|| fail("unreadable Coinbase price"))?,
            size,
        ))
    } else {
        Ok((size.clone(), &price * &size))
    }
}
/// Exchanges::Coinbase#normalize_coinbase_fill, including both crypto swap legs and adjusted-fill ids (C5).
pub fn fill(raw: Raw) -> Result<Vec<Entry>, Fail> {
    let product = raw.value["product_id"].as_str().unwrap_or("");
    let mut parts = product.split('-');
    let base = parts.next().unwrap_or("");
    let quote = parts.next().unwrap_or("");
    let buy = raw.value["side"] == "BUY";
    let (size, total) = amounts(&raw)?;
    let fee = raw.decimal("commission").map_err(fail)?;
    let adjusted = raw.value["trade_type"]
        .as_str()
        .is_some_and(|s| !s.is_empty() && s != "FILL");
    let id = if adjusted {
        text(raw.value.get("entry_id"))
    } else {
        None
    }
    .or_else(|| text(raw.value.get("trade_id")))
    .or_else(|| text(raw.value.get("entry_id")));
    let at = time(&raw.value["trade_time"])?;
    if FIAT.contains(&quote) {
        let mut e = entry(if buy { BUY } else { SELL }, base, size, &raw, at);
        e.quote_currency = Some(quote.into());
        e.quote_amount = Some(total);
        e.fee_currency = Some(quote.into());
        e.fee_amount = Some(fee);
        e.tx_id = id;
        Ok(vec![e])
    } else {
        let id = id.unwrap_or_default();
        let mut a = entry(if buy { SWAP_IN } else { SWAP_OUT }, base, size, &raw, at);
        let mut b = entry(if buy { SWAP_OUT } else { SWAP_IN }, quote, total, &raw, at);
        a.tx_id = Some(format!("{id}-in"));
        b.tx_id = Some(format!("{id}-out"));
        a.group_id = Some(format!("swap_{id}"));
        b.group_id = a.group_id.clone();
        if fee.is_positive() {
            let e = if buy { &mut a } else { &mut b };
            e.fee_currency = Some(quote.into());
            e.fee_amount = Some(fee);
        }
        Ok(vec![a, b])
    }
}
fn transfer(raw: Raw) -> Result<Option<Entry>, Fail> {
    let kind = match raw.value["type"].as_str() {
        Some("send") => WITHDRAWAL,
        Some("receive") => DEPOSIT,
        _ => return Ok(None),
    };
    let Some(amount) = raw.object("amount").map_err(fail)? else {
        return Ok(None);
    };
    let Some(n) = amount.number("amount").map_err(fail)? else {
        return Ok(None);
    };
    let Some(currency) = amount.value["currency"].as_str() else {
        return Ok(None);
    };
    let mut e = entry(
        kind,
        currency,
        if n < BigDec::zero() {
            &BigDec::zero() - &n
        } else {
            n
        },
        &raw,
        time(&raw.value["created_at"])?,
    );
    if let Some(fee) = raw
        .object("network")
        .map_err(fail)?
        .map(|n| n.object("transaction_fee"))
        .transpose()
        .map_err(fail)?
        .flatten()
    {
        if let Some(n) = fee
            .number("amount")
            .map_err(fail)?
            .filter(|n| n.is_positive())
        {
            e.fee_amount = Some(n);
            e.fee_currency = text(fee.value.get("currency"));
        }
    }
    e.tx_id = text(raw.value.get("id"));
    e.description = text(raw.value.get("description"));
    Ok(Some(e))
}
struct Walk<'a, T: Transport> {
    venue: &'a CoinbaseVenue<T>,
    start: Option<String>,
    source: String,
    cursor: Option<String>,
    accounts: Vec<String>,
    account: usize,
}
impl<'a, T: Transport> Walk<'a, T> {
    fn new(venue: &'a CoinbaseVenue<T>, begin: continuation::Begin<'_>) -> Self {
        let mut w = Self {
            venue,
            start: begin
                .start
                .map(|s| s.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            source: "fills".into(),
            cursor: None,
            accounts: vec![],
            account: 0,
        };
        if let Some(c) = begin.cursor {
            w.source = c["source"].as_str().unwrap_or("fills").into();
            w.cursor = c["cursor"].as_str().map(str::to_string);
            w.accounts = c["accounts"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| a.as_str().map(str::to_string))
                .collect();
            w.account = c["account"].as_u64().unwrap_or(0) as usize;
        }
        w
    }
}
impl<T: Transport> continuation::Walk for Walk<'_, T> {
    fn done(&self) -> bool {
        self.source == "done"
    }
    fn cursor(&self) -> Value {
        json!({"source":self.source,"cursor":self.cursor,"accounts":self.accounts,"account":self.account})
    }
    fn page_rows(&self) -> usize {
        2000
    }
    fn request_reserve(&self) -> std::time::Duration {
        crate::venue::http::TOTAL_TIMEOUT
    }
    async fn step(&mut self) -> Result<Vec<Entry>, Fail> {
        let mut q = vec![];
        let (path, field) = match self.source.as_str() {
            "fills" => {
                if let Some(s) = &self.start {
                    q.push(("start_sequence_timestamp", s.clone()));
                }
                (FILLS.to_string(), "fills")
            }
            "accounts" => {
                q.push(("limit", "250".into()));
                (ACCOUNTS.to_string(), "accounts")
            }
            "transfers" => {
                q.push(("limit", "100".into()));
                (
                    format!(
                        "/v2/accounts/{}/transactions",
                        self.accounts
                            .get(self.account)
                            .ok_or_else(|| fail("unreadable Coinbase account cursor"))?
                    ),
                    "data",
                )
            }
            _ => return Ok(vec![]),
        };
        if let Some(c) = &self.cursor {
            q.push((
                if self.source == "transfers" {
                    "starting_after"
                } else {
                    "cursor"
                },
                c.clone(),
            ));
        }
        let body = self
            .venue
            .read_text(&path, q, 8 * 1024 * 1024)
            .await
            .map_err(|m| (m, false))?;
        let node = page_of(&body, "Coinbase ledger", 400_000)?;
        let raw = Raw::from_node(&node).map_err(fail)?;
        let page = match raw.member(field) {
            None => vec![],
            Some(super::wire::Node::Scalar(t)) if t == "null" => vec![],
            Some(n) => objects(n, "Coinbase ledger rows")?,
        };
        let empty = page.is_empty();
        let mut entries = vec![];
        for r in page {
            match self.source.as_str() {
                "fills" => entries.extend(fill(r)?),
                "accounts" => {
                    let id = r.value["uuid"]
                        .as_str()
                        .ok_or_else(|| fail("unreadable Coinbase account uuid"))?;
                    if !self.accounts.iter().any(|a| a == id) {
                        self.accounts.push(id.into());
                    }
                }
                _ => {
                    if let Some(e) = transfer(r)? {
                        entries.push(e)
                    }
                }
            }
        }
        let next = if self.source == "transfers" {
            raw.value["pagination"]["next_starting_after"].as_str()
        } else {
            raw.value["cursor"].as_str()
        }
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string);
        let more = next.is_some()
            && if self.source == "accounts" {
                raw.value["has_next"] == true
            } else {
                !empty
            };
        if more {
            if next == self.cursor {
                return Err(fail("Coinbase ledger paging stuck"));
            }
            self.cursor = next;
        } else {
            self.cursor = None;
            match self.source.as_str() {
                "fills" => self.source = "accounts".into(),
                "accounts" => {
                    self.source = if self.accounts.is_empty() {
                        "done"
                    } else {
                        "transfers"
                    }
                    .into()
                }
                _ => {
                    self.account += 1;
                    if self.account >= self.accounts.len() {
                        self.source = "done".into();
                    }
                }
            }
        }
        Ok(entries)
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
            load_key_of(c, key_id, &[COINBASE])?,
            super::cache::ledger_produced_by(c, key_id, &captured)?,
        ))
    })
    .await?;
    let watermark = if produced { key.last_synced_at } else { None };
    let venue = CoinbaseVenue::new(transport, URL);
    continuation::sync(
        db,
        &version,
        key,
        watermark,
        Duration::hours(25),
        credentials,
        clock.now(),
        run,
        |b| Walk::new(&venue, b),
    )
    .await
}
