//! AccountTransaction::SyncJob for an Alpaca key: Exchanges::Alpaca#get_ledger (every page of
//! GET /v2/account/activities), AccountTransactionSync#sync! (dedup, rows, the bot's own order, split side effects,
//! asset identity, the watermark) and TransferMatcher.run!. Rails is the oracle (rust/tests/sync_parity.rs).
use super::activities::{self, cash_activity, CryptoPair, CryptoPairs, Entry, Raw, ADJUSTMENT, BUY, SELL};
use crate::jobs::Db;
use super::wire::{self, Budget, Node};
use super::{commit, commit_bots, load_key, number, parsed, phase, record_sync_error, sql_time, Failure, Key, SyncError, Unread, LIVE_REFUSED};
use crate::codec::parse_time;
use crate::crypto::Credentials;
use crate::engine::Clock;
use crate::ruby::BigDec;
use crate::venue::alpaca::AlpacaVenue;
use crate::venue::http::Transport;
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

pub const ACTIVITIES_PATH: &str = "/v2/account/activities";
/// Alpaca's maximum, and what Rails asks for. A page shorter than this is the last.
pub const PAGE_SIZE: usize = 100;
/// The overlap re-read on every incremental sync, so entries posted late (a dividend after its pay date) still land.
pub const OVERLAP_HOURS: i64 = 25;
/// AccountTransactionSync::ASSET_CATCH_UP: how long a stored row's asset may still be looked up.
pub const ASSET_CATCH_UP_DAYS: i64 = 7;
/// BotActivityLog::PruneJob::RETENTION: a split older than this is not announced.
pub const FEED_RETENTION_DAYS: i64 = 90;
/// TransferMatcher::WINDOW.
pub const TRANSFER_WINDOW_HOURS: i64 = 72;
/// Rows per unit on the blocking pool, a read or one write transaction: the engine can take the write lock between two
/// (`sync::WRITE_GAP`). A split row is a unit of its own.
pub const BATCH: usize = 100;
/// What one run reads of the ledger. A run that reaches `MAX_PAGES` stores what it read and leaves the rest to the
/// next run (`Import`); past any other limit the run fails, and what earlier runs stored stays:
/// - one page's body, in bytes (a page of 100 activities is about 60 KB), and its values and keys (about 3,000);
/// - the items of one page: `PAGE_SIZE`, what was asked for, refused at the 101st while the page is read;
/// - the pages of one run, and so its activities (50,000) and its values and keys;
/// - the legs of one split, when a run at its cap holds nothing but that split and reads on to its end (`fetch`).
pub const MAX_PAGE_BYTES: usize = 256 * 1024;
pub const MAX_PAGE_NODES: usize = 20_000;
pub const MAX_PAGES: usize = 500;
pub const MAX_ACTIVITIES: usize = MAX_PAGES * PAGE_SIZE;
pub const MAX_RUN_NODES: usize = 3_000_000;
/// The one bound on reading past a run's pages, which a run does only to finish the split it holds nothing but: at
/// most `MAX_READ_ON_PAGES` more pages (every activity on them counts, cancelled ones and other kinds included), and
/// that split may not grow past `MAX_SPLIT_LEGS` legs, checked on every page as it is appended, a short last page
/// included. Five pages hold the 500 legs with room for what ends the split. A real split has two or three legs.
pub const MAX_READ_ON_PAGES: usize = 5;
pub const MAX_SPLIT_LEGS: usize = 500;
/// The runs one import may take before it is given up as not ending: 5,000,000 activities at the jobs' cap.
pub const MAX_IMPORT_RUNS: usize = 100;

/// What bounds one run, and one import. `RUN` is what the jobs use; a test states fewer pages and runs so that an
/// import of a few hundred activities takes several runs, or too many.
#[derive(Clone, Copy, Debug)]
pub struct Limits { pub pages: usize, pub runs: usize }
impl Limits { pub const RUN: Limits = Limits { pages: MAX_PAGES, runs: MAX_IMPORT_RUNS }; }

/// Exchanges::Alpaca::CRYPTO_COINGECKO_IDS: the curated map behind a coin the venue lists no pair for.
pub const CRYPTO_COINGECKO_IDS: [(&str, &str); 36] = [
    ("AAVE", "aave"), ("ADA", "cardano"), ("ARB", "arbitrum"), ("AVAX", "avalanche-2"), ("BAT", "basic-attention-token"), ("BCH", "bitcoin-cash"),
    ("BONK", "bonk"), ("BTC", "bitcoin"), ("CRV", "curve-dao-token"), ("DOGE", "dogecoin"), ("DOT", "polkadot"), ("ETH", "ethereum"),
    ("FIL", "filecoin"), ("GRT", "the-graph"), ("HYPE", "hyperliquid"), ("LDO", "lido-dao"), ("LINK", "chainlink"), ("LTC", "litecoin"),
    ("ONDO", "ondo-finance"), ("PAXG", "pax-gold"), ("PEPE", "pepe"), ("POL", "polygon-ecosystem-token"), ("RENDER", "render-token"),
    ("SHIB", "shiba-inu"), ("SKY", "sky"), ("SOL", "solana"), ("SUSHI", "sushi"), ("TRUMP", "official-trump"), ("UNI", "uniswap"),
    ("USDC", "usd-coin"), ("USDG", "global-dollar"), ("USDT", "tether"), ("WIF", "dogwifcoin"), ("XRP", "ripple"), ("XTZ", "tezos"), ("YFI", "yearn-finance"),
];
/// Exchanges::Alpaca::CRYPTO_QUOTES, in its order.
pub const CRYPTO_QUOTES: [&str; 4] = ["USD", "USDT", "USDC", "BTC"];
/// Tax::PriceService::FIAT_CURRENCIES.
pub const FIAT_CURRENCIES: [&str; 13] = ["USD", "EUR", "GBP", "CHF", "SEK", "PLN", "DKK", "CZK", "BGN", "AUD", "CAD", "JPY", "AED"];
/// MarketData::TICKER_TOMBSTONE_PREFIX: a replaced listing's base is "__stale_<id>_<SPELLING>".
const TOMBSTONE: &str = "__stale_";

/// A split row this run stored, and what it did for the bots that traded the symbol.
#[derive(Clone, Debug, PartialEq)]
pub struct Split {
    pub symbol: String,
    pub at: DateTime<Utc>,
    /// Bots whose `restatement_generation` moved (every bot of the user that traded the symbol on this venue).
    pub restated_bots: Vec<i64>,
    /// The split is dated ahead of now. Rails books Bot::ExpireRestatedMetricsJob for `at`; here the bump owed at `at`
    /// is recorded with the row (`Owed`) and made by the first ledger sync of the key at or after it.
    pub effective_later: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub imported: usize,
    pub duplicates: usize,
    pub skipped: usize,
    /// `api_keys.last_synced_at` after the run: the newest transaction time the venue returned, held back by a row
    /// that could not be saved and by the moment the sync began; unchanged when the venue returned nothing.
    pub watermark: Option<DateTime<Utc>>,
    pub splits: Vec<Split>,
    /// Withdrawals TransferMatcher linked to a deposit.
    pub linked: usize,
    /// False when the run stopped at its page cap with more of the ledger to read: what it read is stored, the
    /// watermark has not moved, and the next run goes on from where this one stopped (`Import`).
    pub complete: bool,
}

/// An import that one run did not finish: `app_configs` row `rust_sync.ledger:<api_key_id>`, plain JSON. Rails never
/// reads it (after a handback it is inert: Rails syncs from `api_keys.last_synced_at`, which an unfinished import has
/// not moved, and reads the rows already stored as duplicates). A row whose `watermark` is not the key's
/// `last_synced_at` any more was left behind such a handback, and is ignored.
#[derive(Clone, Debug, PartialEq)]
struct Import {
    /// The last activity id stored: the next run's first `page_token`.
    cursor: String,
    /// Where each earlier run of this import stopped, the last one's (`cursor`) included: an import that comes to
    /// one of them again is going round, and is stopped.
    cursors: Vec<String>,
    /// The runs this import has taken so far, counted as each starts (so a run that fails, or is dropped, counts),
    /// and the pages it has stored.
    runs: usize,
    pages: usize,
    /// The `after` the import asks with, on every run.
    after: Option<String>,
    /// When the import's first run began: what the watermark may not pass.
    started: DateTime<Utc>,
    max_seen: Option<DateTime<Utc>>,
    min_skipped: Option<DateTime<Utc>>,
    stored: usize,
    /// The key's `last_synced_at` when the import began (microseconds).
    watermark: Option<i64>,
}

fn import_key(key_id: i64) -> String { format!("rust_sync.ledger:{key_id}") }
fn micros(t: Option<DateTime<Utc>>) -> Option<i64> { t.map(|t| t.timestamp_micros()) }

/// Whether the key has an import record at all, and the import it describes when it is still about this state of the
/// key. A record that is not (Rails moved the watermark since, or the row is not this port's JSON) is ignored, and
/// removed by the run that completes.
fn load_import(c: &Connection, key: &Key) -> Result<(bool, Option<Import>), SyncError> {
    let raw: Option<Option<String>> = c.query_row("SELECT value FROM app_configs WHERE key = ?1", [import_key(key.id)], |r| r.get(0)).optional()?;
    let Some(raw) = raw else { return Ok((false, None)) };
    let Some(v) = raw.and_then(|text| serde_json::from_str::<Value>(&text).ok()) else { return Ok((true, None)) };
    let time = |k: &str| v[k].as_i64().and_then(DateTime::<Utc>::from_timestamp_micros);
    let (Some(cursor), Some(started)) = (v["cursor"].as_str(), time("started")) else { return Ok((true, None)) };
    if v["watermark"].as_i64() != micros(key.last_synced_at) { return Ok((true, None)); }
    let cursors = v["cursors"].as_array().into_iter().flatten().filter_map(|c| c.as_str().map(str::to_string)).collect();
    let count = |k: &str| v[k].as_u64().unwrap_or(0) as usize;
    Ok((true, Some(Import { cursor: cursor.to_string(), cursors, runs: count("runs"), pages: count("pages"), after: v["after"].as_str().map(str::to_string), started,
                            max_seen: time("max_seen"), min_skipped: time("min_skipped"), stored: count("stored"), watermark: v["watermark"].as_i64() })))
}

fn save_import(c: &Connection, key_id: i64, import: &Import, now: DateTime<Utc>) -> Result<(), SyncError> {
    let value = json!({ "cursor": import.cursor, "cursors": import.cursors, "runs": import.runs, "pages": import.pages, "after": import.after,
                        "started": import.started.timestamp_micros(), "max_seen": micros(import.max_seen), "min_skipped": micros(import.min_skipped),
                        "stored": import.stored, "watermark": import.watermark });
    put_config(c, &import_key(key_id), &value, now)
}

fn put_config(c: &Connection, key: &str, value: &Value, now: DateTime<Utc>) -> Result<(), SyncError> {
    c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES (?1, ?2, ?3, ?3) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
              params![key, value.to_string(), sql_time(now)])?;
    Ok(())
}

/// A bump owed at the date of a split imported ahead of it: Rails' Bot::ExpireRestatedMetricsJob, which `log_split`
/// books in Solid Queue for the split's `transacted_at` and which then runs `expire_restated_bots(user, exchange,
/// symbol)` once. Kept in the sync's own `app_configs` row `rust_sync.ledger_splits:<api_key_id>`, plain JSON
/// (`{"pending":[{"symbol","exchange_id","at":<microseconds>},…]}`), written in the split row's own transaction, and
/// made by the first ledger sync of the key at or after `at` (`expire_owed`), which removes it in the same unit. A
/// separate row from the import record, which a completed run deletes; one entry per split row stored, as Rails books
/// one job per row. After a handback the row is inert: Rails never reads it, and since Rust stored the split Rails'
/// own sync reads it as a duplicate and books nothing, so a date that passes while Rails runs gets no second bump
/// there; the first Rust ledger sync after the next takeover makes it.
#[derive(Clone, Debug, PartialEq)]
struct Owed { symbol: String, exchange_id: i64, at: i64 }

fn splits_key(key_id: i64) -> String { format!("rust_sync.ledger_splits:{key_id}") }

fn load_owed(c: &Connection, key_id: i64) -> Result<Vec<Owed>, SyncError> {
    let raw: Option<Option<String>> = c.query_row("SELECT value FROM app_configs WHERE key = ?1", [splits_key(key_id)], |r| r.get(0)).optional()?;
    let v = raw.flatten().and_then(|text| serde_json::from_str::<Value>(&text).ok()).unwrap_or(Value::Null);
    Ok(v["pending"].as_array().into_iter().flatten()
        .filter_map(|o| Some(Owed { symbol: o["symbol"].as_str()?.to_string(), exchange_id: o["exchange_id"].as_i64()?, at: o["at"].as_i64()? })).collect())
}

fn save_owed(c: &Connection, key_id: i64, owed: &[Owed], now: DateTime<Utc>) -> Result<(), SyncError> {
    if owed.is_empty() { c.execute("DELETE FROM app_configs WHERE key = ?1", [splits_key(key_id)])?; return Ok(()); }
    let pending: Vec<Value> = owed.iter().map(|o| json!({ "symbol": o.symbol, "exchange_id": o.exchange_id, "at": o.at })).collect();
    put_config(c, &splits_key(key_id), &json!({ "pending": pending }), now)
}

/// Every bump owed at or before `now`, each a read unit (which bots the symbol names now, as the Rails job reads them
/// when it runs) and one guarded write unit that bumps them and removes the entry. A refusal fails the sync and leaves
/// the entry owed.
async fn expire_owed(db: &Db, key_id: i64, user_id: i64, now: DateTime<Utc>) -> Result<(), SyncError> {
    let due: Vec<Owed> = phase(db, move |c| Ok(load_owed(c, key_id)?.into_iter().filter(|o| o.at <= now.timestamp_micros()).collect())).await?;
    for owed in due {
        let o = owed.clone();
        let bots = phase(db, move |c| bots_naming(c, user_id, o.exchange_id, &o.symbol)).await?;
        commit_bots(db, move |c| {
            bump(c, &bots)?;
            let mut rest = load_owed(c, key_id)?;
            if let Some(i) = rest.iter().position(|r| *r == owed) { rest.remove(i); }
            save_owed(c, key_id, &rest, now)?;
            Ok(((), !bots.is_empty()))
        }).await?;
    }
    Ok(())
}

/// One ledger sync of one Alpaca key, as AccountTransaction::SyncJob#perform runs it, within the jobs' limits.
pub async fn sync<T: Transport>(db: &Db, venue: &AlpacaVenue<T>, key_id: i64, credentials: &Credentials, clock: &dyn Clock) -> Result<Result<Outcome, Failure>, SyncError> {
    sync_within(db, venue, key_id, credentials, clock, Limits::RUN).await
}

/// One ledger sync of one Alpaca key. Idempotent: a second run over the same activities stores nothing and writes
/// nothing.
///
/// `Ok(Err(failure))` is a run that failed: nothing of what it fetched is stored (Rails fetches every page before
/// it stores one row), the watermark is untouched, and the key's `last_sync_error` holds `failure.error`.
/// `Err` is this process failing (SQLite, a row Rails did not write, the engine's guard): rows of units already
/// committed stay, the watermark does not move, and the next run reads them as duplicates.
///
/// A run reads at most `limits.pages` pages. When the ledger goes on past them, the run stores what it read, records
/// where it stopped (`Import`) and returns `complete: false` without moving the watermark; the next run goes on from
/// there, and the run that reaches the end writes the watermark. However many runs it took, the rows are the ones a
/// single read would have stored.
///
/// Every database step is one bounded unit on the blocking pool with an await after it: a read, or one write
/// transaction of at most `BATCH` rows (a split row alone, with what it does to the bots computed by the read before
/// it), `WRITE_GAP` apart. A stop or a deadline drops the run between two units.
///
/// The watermark never passes the moment the sync began (as Rails'). Deliberately not Rails' (a listed divergence, with
/// Rails' result beside it in the grid): a page token that comes back raises (Rails returns the same failure). Split rows are Rails' own, to the
/// row: its grouping of consecutive legs, its dedup by the group's first id.
pub async fn sync_within<T: Transport>(db: &Db, venue: &AlpacaVenue<T>, key_id: i64, credentials: &Credentials, clock: &dyn Clock, limits: Limits)
                                       -> Result<Result<Outcome, Failure>, SyncError> {
    let now = clock.now();
    let (key, (_, resumed)) = phase(db, move |c| { let key = load_key(c, key_id)?; let import = load_import(c, &key)?; Ok((key, import)) }).await?;
    let key = Arc::new(key);
    let user_id = key.user_id;
    expire_owed(db, key_id, user_id, now).await?;
    let fail = |text: String, raised: bool| {
        let (creds, now) = (credentials.clone(), clock.now());
        async move {
            let error = commit(db, move |c| record_sync_error(c, key_id, &text, &creds)).await?;
            // A returned Failure lets the job go on to the transfer matcher; a raise ends it.
            if !raised { link_transfers(db, user_id, now).await?; }
            Ok::<_, SyncError>(Err(Failure { error, raised }))
        }
    };
    if super::live(credentials) { return fail(LIVE_REFUSED.into(), true).await; }

    // Where this run starts: where an unfinished import stopped, or the watermark less the overlap. A stored
    // watermark ahead of now (Rails left one behind a split dated ahead, before it capped the watermark too) is read as now.
    let import = resumed.unwrap_or_else(|| Import { cursor: String::new(), cursors: vec![], runs: 0, pages: 0, after: key.last_synced_at.and_then(|w| after(w.min(now))),
                                                    started: now, max_seen: None, min_skipped: None, stored: 0, watermark: micros(key.last_synced_at) });
    // An import that does not end is stopped: its record goes, the watermark stays, and the key and the job say why.
    let give_up = |text: &'static str| async move {
        commit(db, move |c| Ok(c.execute("DELETE FROM app_configs WHERE key = ?1", [import_key(key_id)])?)).await?;
        fail(text.to_string(), true).await
    };
    if import.runs >= limits.runs { return give_up("the ledger import did not end within its runs: it was stopped, and starts again at the next sync").await; }
    // Every run is counted in the record before it reads anything, the first run of an import too, so one that fails
    // or is dropped counts: an import that fails at the same place every time comes to its last run and is ended.
    // The run that completes removes the record.
    let continued = !import.cursor.is_empty();
    let import = Import { runs: import.runs + 1, ..import };
    let counted = import.clone();
    commit(db, move |c| save_import(c, key_id, &counted, now)).await?;
    let started = import.started;
    // Fetch: no database handle in reach, so no transaction can be open across a request.
    let cursor = (!import.cursor.is_empty()).then(|| import.cursor.clone());
    let fetched = match fetch(venue, import.after.clone(), cursor, &import.cursors, limits).await {
        Ok(fetched) => fetched,
        // A page token this import has already passed, in this run or an earlier one: it is going round.
        Err((text, _)) if text == TOKENS_REPEAT && continued => return give_up("the ledger import met a page token it had already passed: it was stopped, and starts again at the next sync").await,
        Err((text, raised)) => return fail(text, raised).await,
    };
    let activities = Arc::new(fetched.activities);
    let prepared = { let (key, activities) = (key.clone(), activities.clone()); phase(db, move |c| prepare(c, &key, &activities)).await? };
    let (entries, mut progress) = match prepared {
        Ok(prepared) => prepared,
        // Ruby raises on a time it cannot read (Time.parse, nil.utc) and on a split ratio that is not finite
        // (FloatDomainError), and the job re-raises.
        Err(text) => return fail(text, true).await,
    };
    let entries = Arc::new(entries);
    let mut from = 0;
    while from < entries.len() {
        let (key, batch) = (key.clone(), entries.clone());
        if split(&entries[from]) {
            let effects = { let (key, batch) = (key.clone(), batch.clone()); phase(db, move |c| split_effects(c, &key, &batch[from], now)).await? };
            // A split's unit, alone: past the engine's guard whether or not it moves a counter, since eligibility reads the
            // account's split rows (`eligibility::history_reasons`).
            progress = commit_bots(db, move |c| Ok((store(c, &key, &batch[from..from + 1], Some(&effects), progress, now)?, true))).await?;
            from += 1;
        } else {
            let end = entries.len().min(from + BATCH);
            let to = (from..end).find(|i| split(&entries[*i])).unwrap_or(end);
            progress = commit(db, move |c| store(c, &key, &batch[from..to], None, progress, now)).await?;
            from = to;
        }
    }
    // #resolve_recent_assets, a page of candidates at a time.
    let mut after_id = 0;
    loop {
        let (key, listings) = (key.clone(), progress.listings.clone());
        let page = phase(db, move |c| unresolved_assets(c, &key, &listings, now, after_id)).await?;
        if !page.found.is_empty() { commit(db, move |c| set_assets(c, &page.found)).await?; }
        match page.next { Some(id) => after_id = id, None => break }
    }
    let mut outcome = progress.out;
    let max_seen = [import.max_seen, entries.iter().filter_map(|e| e.transacted_at).max()].into_iter().flatten().max();
    let min_skipped = [import.min_skipped, progress.min_skipped].into_iter().flatten().min();
    if let (true, Some(cursor)) = (fetched.more, fetched.cursor) {
        // The page cap: what was read is stored; where it stopped is recorded; the watermark waits for the end.
        let mut cursors = import.cursors.clone();
        cursors.push(cursor.clone());
        let next = Import { cursor, cursors, pages: import.pages + fetched.pages, max_seen, min_skipped, stored: import.stored + outcome.imported, ..import };
        crate::engine::log(&format!("[alpaca] ledger import of api key {key_id} is not complete: {} activities stored so far, the next run continues", next.stored));
        commit(db, move |c| save_import(c, key_id, &next, now)).await?;
        return Ok(Ok(outcome));
    }
    // The watermark comes from the data, never the clock, and never passes a row that failed to save. Nor does it
    // pass the moment this sync (or the import it finishes) began (as Rails'): a row dated ahead (a pending split) would
    // carry it into the future, and every activity before that date would then be outside every later window.
    let watermark = [max_seen, min_skipped].into_iter().flatten().min().or(key.last_synced_at).map(|w| w.min(started));
    // `update!`: nothing is written, and updated_at does not move, when neither column changes.
    let changed = micros(watermark) != micros(key.last_synced_at) || key.last_sync_error.is_some();
    // The import's record goes when a run completes: this run's count, an import's, or one left behind a handback.
    {
        commit(db, move |c| {
            if changed { c.execute("UPDATE api_keys SET last_synced_at = ?1, last_sync_error = NULL, updated_at = ?2 WHERE id = ?3", params![watermark.map(sql_time), sql_time(now), key_id])?; }
            c.execute("DELETE FROM app_configs WHERE key = ?1", [import_key(key_id)])?;
            Ok(())
        }).await?;
    }
    outcome.watermark = watermark;
    outcome.complete = true;
    outcome.linked = link_transfers(db, user_id, clock.now()).await?;
    Ok(Ok(outcome))
}

/// The `after` an incremental sync asks for: the watermark less the 25-hour overlap, as `Time#iso8601` (whole seconds).
/// A key with no watermark sends none and reads the whole history. Alpaca caps no window, so there is no floor.
/// `None` for a watermark with no time 25 hours before it (one at the start of chrono's calendar): the whole history.
pub fn after(last_synced_at: DateTime<Utc>) -> Option<String> {
    last_synced_at.checked_sub_signed(Duration::hours(OVERLAP_HOURS)).map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// One page as #get_ledger reads it (`Array(result.data)`: an empty object or null is no activities), each activity
/// as Rails would hold it. At most `PAGE_SIZE` items and `nodes` values and keys are read; the second number is what
/// the page spent of them.
fn page(text: &str, nodes: usize) -> Result<(Vec<Raw>, usize), Unread> {
    let mut budget = Budget(nodes);
    let items = match wire::read(text, &mut budget, Some(PAGE_SIZE)).map_err(|r| Unread::refused(r, "an activities page"))? {
        Node::Array(items) => items,
        Node::Scalar(s) if s == "null" => vec![],
        Node::Object(members) if members.is_empty() => vec![],
        _ => return Err(Unread::Raised("unreadable activities page".into())),
    };
    let activities = items.iter().map(|item| Raw::from_node(item).map_err(|_| Unread::Raised("unreadable activity".into()))).collect::<Result<Vec<_>, _>>()?;
    Ok((activities, nodes - budget.0))
}

/// What one run read: the activities it will store, the id of the last of them, how many pages it took, and whether
/// the ledger goes on past the run's pages.
struct Fetched { activities: Vec<Raw>, cursor: Option<String>, pages: usize, more: bool }

const TOKENS_REPEAT: &str = "the ledger's page tokens repeat: nothing was read";

const SPLIT_TOO_LONG: &str = "a split longer than a run of the ledger import may read on for: nothing was read";

/// #get_ledger's loop, for one run: `direction=asc` (oldest first, so what a run stores is final and the next run
/// continues after it), `page_size=100`, the last id of a page as the next `page_token` (Alpaca's exclusive cursor),
/// `after` when incremental. Ends on an empty page or a short page; stops, with more to read, after `limits.pages`
/// full pages. Fails, with nothing read, on a page over `MAX_PAGE_BYTES`, `PAGE_SIZE` items or its budget of values,
/// on a page token met before in any position, in this run or as the stop of an earlier run of the same import
/// (`passed`; Rails fails on one met before within its single read), or on a full page
/// whose last activity has no id. Each page is parsed on the blocking pool.
///
/// A run that stops at its cap does not cut a split in two. Rails merges split legs that are neighbours in one read,
/// after it has dropped the cancelled activities, and it reads the whole ledger at once; so the stop is decided on
/// that same sequence (`activities::open_split`):
/// - a read that does not end in a split leg stops where it is;
/// - one that does leaves the group it ends in, and only that group, for the next run: the cursor is the activity
///   before the group's first leg, whatever kind it is;
/// - when that group is all the run holds (so leaving it would leave everything), the run reads on, a page at a time,
///   until the group ends, and then stops after it: at most `MAX_READ_ON_PAGES` pages past its own, and the group
///   at most `MAX_SPLIT_LEGS` legs, checked on every page it appends. Past either the run fails.
async fn fetch<T: Transport>(venue: &AlpacaVenue<T>, start: Option<String>, cursor: Option<String>, passed: &[String], limits: Limits) -> Result<Fetched, (String, bool)> {
    let mut all: Vec<Raw> = vec![];
    let mut seen: HashSet<String> = passed.iter().chain(cursor.iter()).cloned().collect();
    let mut page_token = cursor;
    let mut nodes = MAX_RUN_NODES;
    let mut pages = 0;
    loop {
        let mut query = vec![("direction", "asc".to_string()), ("page_size", PAGE_SIZE.to_string())];
        if let Some(token) = &page_token { query.push(("page_token", token.clone())); }
        if let Some(start) = &start { query.push(("after", start.clone())); }
        let body = venue.read(false, ACTIVITIES_PATH, query, MAX_PAGE_BYTES).await.map_err(super::venue_failure)?;
        let allowed = nodes.min(MAX_PAGE_NODES);
        let (items, spent) = parsed(body, move |text| page(text, allowed)).await?;
        nodes -= spent;
        pages += 1;
        let Some(last) = items.last() else { return Ok(Fetched { activities: all, cursor: page_token, pages, more: false }) };
        let next = last.value["id"].as_str().map(str::to_string);
        // The token is the venue's own value: no failure text and no log line repeats it.
        if let Some(token) = &next {
            if !seen.insert(token.clone()) { return Err((TOKENS_REPEAT.into(), true)); }
        }
        let short = items.len() < PAGE_SIZE;
        all.extend(items);
        // Reading on for one split: the split may not have grown past its bound, whatever this page holds.
        if pages > limits.pages && activities::leading_split(&all) > MAX_SPLIT_LEGS { return Err((SPLIT_TOO_LONG.into(), true)); }
        if short { return Ok(Fetched { activities: all, cursor: next.or(page_token), pages, more: false }); }
        if next.is_none() { return Err(("a full page of the ledger ends with an activity that has no id: nothing was read".into(), true)); }
        page_token = next;
        if pages < limits.pages { continue; }
        // The cap, or past it for the sake of one split.
        match activities::open_split(&all) {
            None => return Ok(Fetched { activities: all, cursor: page_token, pages, more: true }),
            Some((0, legs)) if legs > MAX_SPLIT_LEGS || pages >= limits.pages + MAX_READ_ON_PAGES => return Err((SPLIT_TOO_LONG.into(), true)),
            Some((0, _)) => {} // nothing but one split so far: read on to its end, a page at a time
            Some((first_leg, _)) => {
                let Some(before) = all[first_leg - 1].value["id"].as_str().map(str::to_string) else {
                    return Err(("a run of the ledger import ends in a split it cannot leave for the next run: nothing was read".into(), true));
                };
                all.truncate(first_leg);
                return Ok(Fetched { activities: all, cursor: Some(before), pages, more: true });
            }
        }
    }
}

/// #crypto_position_index: available tickers whose base asset is a coin, by "BASEQUOTE". The last ticker wins a name.
pub fn crypto_pairs(c: &Connection, exchange_id: i64) -> Result<CryptoPairs, SyncError> {
    let mut s = c.prepare(
        "SELECT t.base, t.quote, a.symbol, t.base_asset_id FROM tickers t JOIN assets a ON a.id = t.base_asset_id \
         WHERE t.exchange_id = ?1 AND t.available = 1 AND a.category = 'Cryptocurrency' ORDER BY t.id")?;
    let rows = s.query_map([exchange_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, i64>(3)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(CryptoPairs(rows.into_iter().map(|(base, quote, asset_symbol, base_asset_id)| (format!("{base}{quote}"), CryptoPair { base, quote, asset_symbol, base_asset_id })).collect()))
}

fn blank(s: &str) -> bool { s.chars().all(char::is_whitespace) }
fn present(s: &Option<String>) -> Option<&str> { s.as_deref().filter(|s| !blank(s)) }
/// How ActiveRecord's SQLite adapter binds a BigDecimal: its nearest double.
fn real(d: &BigDec) -> f64 { d.to_f() }

/// [coins, securities], each { SPELLING => [asset ids] }: #ledger_listings over every listing of the venue (Rails
/// narrows the query to the names in hand; the lookups below only ever ask for those).
struct Listings { coins: HashMap<String, Vec<i64>>, securities: HashMap<String, Vec<i64>> }

fn spelling(base: &str) -> String {
    // Ticker#base_spelling: without "__stale_<digits>_".
    let rest = base.strip_prefix(TOMBSTONE).and_then(|r| { let d = r.chars().take_while(char::is_ascii_digit).count(); (d > 0).then(|| r[d..].strip_prefix('_')).flatten() });
    rest.unwrap_or(base).to_uppercase()
}

fn listings(c: &Connection, exchange_id: i64) -> Result<Listings, SyncError> {
    let mut s = c.prepare("SELECT t.base, a.category, t.base_asset_id FROM tickers t JOIN assets a ON a.id = t.base_asset_id WHERE t.exchange_id = ?1 ORDER BY t.id")?;
    let rows = s.query_map([exchange_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
    let mut l = Listings { coins: HashMap::new(), securities: HashMap::new() };
    for (base, category, asset_id) in rows {
        let ids = if category.as_deref() == Some("Cryptocurrency") { &mut l.coins } else { &mut l.securities }.entry(spelling(&base)).or_default();
        if !ids.contains(&asset_id) { ids.push(asset_id); }
    }
    Ok(l)
}

fn only(ids: Option<&Vec<i64>>) -> Option<i64> { ids.filter(|i| i.len() == 1).map(|i| i[0]) }
fn curated(coin: &str) -> Option<&'static str> { CRYPTO_COINGECKO_IDS.iter().find(|(k, _)| *k == coin).map(|(_, id)| *id) }

/// Exchanges::Alpaca#ledger_asset_ids for one row: the activity decides. Its type says cash; its raw symbol says coin
/// or security; a name two listings share stays NULL.
fn asset_id(c: &Connection, l: &Listings, base_currency: Option<&str>, raw: &Value) -> Result<Option<i64>, SyncError> {
    let base = base_currency.unwrap_or_default().to_uppercase();
    let Some(kind) = raw.get("activity_type").and_then(Value::as_str) else {
        // A row from a file: no activity behind it. A name Alpaca also trades as a coin could be either.
        if FIAT_CURRENCIES.contains(&base.as_str()) || curated(&base).is_some() || l.coins.contains_key(&base) { return Ok(None); }
        return Ok(only(l.securities.get(&base)));
    };
    if cash_activity(kind) { return Ok(None); }
    let symbol = raw["symbol"].as_str().unwrap_or_default().to_uppercase();
    let trade = kind == "FILL" || kind == "CFEE";
    let coin = trade.then(|| ledger_coin(&symbol, l)).flatten().or_else(|| (kind == "CFEE").then(|| base.clone()));
    match coin {
        // The curated map only where the venue lists no coin by that name: listings that disagree stay NULL.
        Some(coin) if l.coins.contains_key(&coin) => Ok(only(l.coins.get(&coin))),
        Some(coin) => match curated(&coin) {
            Some(external_id) => Ok(c.query_row("SELECT id FROM assets WHERE external_id = ?1 LIMIT 1", [external_id], |r| r.get(0)).optional()?),
            None => Ok(None),
        },
        None => Ok(only(l.securities.get(&symbol))),
    }
}

/// #ledger_coin: the coin a raw crypto symbol names ("BTC/USD", or the compact "BTCUSD"), or None for a security's.
fn ledger_coin(symbol: &str, l: &Listings) -> Option<String> {
    if let Some((coin, _)) = symbol.split_once('/') { return Some(coin.to_string()); }
    CRYPTO_QUOTES.iter().filter_map(|q| symbol.strip_suffix(q)).find(|coin| !blank(coin) && (l.coins.contains_key(*coin) || curated(coin).is_some())).map(str::to_string)
}

/// What one sync carries from unit to unit.
struct Progress {
    out: Outcome,
    listings: Arc<Listings>,
    /// Every leg id of every merged split stored: a standalone leg arriving later is a duplicate.
    merged: HashSet<String>,
    min_skipped: Option<DateTime<Utc>>,
}

/// The read before the store: the activities as entries (#normalize_activity, #merge_split_entries), the venue's
/// listings, and the merged legs already stored (one row per split the account ever had). `Ok(Err(text))`: an
/// activity this port cannot read.
fn prepare(c: &Connection, key: &Key, activities: &[Raw]) -> Result<Result<(Vec<Entry>, Progress), String>, SyncError> {
    let pairs = crypto_pairs(c, key.exchange_id)?;
    let mut entries = Vec::with_capacity(activities.len());
    for a in activities {
        match activities::normalize(a, &pairs) {
            Ok(Some(e)) => entries.push(e),
            Ok(None) => {}
            Err(text) => return Ok(Err(text)),
        }
    }
    let entries = match activities::merge_splits(entries) { Ok(entries) => entries, Err(text) => return Ok(Err(text)) };
    let mut merged: HashSet<String> = HashSet::new();
    let mut s = c.prepare("SELECT raw_data FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND entry_type = ?3")?;
    for raw in s.query_map(params![key.user_id, key.exchange_id, ADJUSTMENT], |r| r.get::<_, Option<String>>(0))? {
        merged.extend(merged_ids(&raw?.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or(Value::Null)));
    }
    Ok(Ok((entries, Progress { out: Outcome::default(), listings: Arc::new(listings(c, key.exchange_id)?), merged, min_skipped: None })))
}

/// A split row, as everything that reacts to one tells it.
fn split(e: &Entry) -> bool { e.entry_type == ADJUSTMENT && e.raw.value["corporate_action"] == "split" }

/// AccountTransactionSync#store! for one unit, inside the caller's write transaction: at most `BATCH` entries, or one
/// split with the `effects` the read before it computed. A split row is stored, skipped and announced exactly as
/// Rails does it: a group is a duplicate when its first leg's id is stored, or is a leg of a stored merged row.
fn store(c: &Connection, key: &Key, batch: &[Entry], effects: Option<&Effects>, mut p: Progress, now: DateTime<Utc>) -> Result<Progress, SyncError> {
    let no_effects = || SyncError("a split row outside a unit of its own".into());
    for (index, e) in batch.iter().enumerate() {
        let tx_id = present(&e.tx_id);
        if duplicate(c, key, e, tx_id, &p.merged)? { p.out.duplicates += 1; continue; }
        // One malformed broker row never aborts the sync: it is logged, counted, and holds the watermark back. The
        // row's id is the venue's own value: the line says which entry of the unit it was, not what it held.
        let (Some(base_currency), Some(at)) = (present(&e.base_currency), e.transacted_at) else {
            crate::engine::log(&format!("[alpaca] Account transaction sync skipped invalid entry {} of {} (entry_type={})", index + 1, batch.len(), e.entry_type));
            p.min_skipped = [p.min_skipped, e.transacted_at].into_iter().flatten().min();
            p.out.skipped += 1;
            continue;
        };
        let no_fee = e.fee_amount.as_ref().is_none_or(BigDec::is_zero);
        let bot_order = if [BUY, SELL, 2, 3].contains(&e.entry_type) { bot_order(c, key.exchange_id, tx_id, &e.raw.value)? } else { None };
        c.execute(
            "INSERT INTO account_transactions (user_id, api_key_id, exchange_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, \
             fee_currency, fee_amount, tx_id, group_id, description, transacted_at, raw_data, manual_values, base_asset_id, transaction_id, \
             transfer_link_rejected, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, '{}', ?16, ?17, 0, ?18, ?18)",
            params![key.user_id, key.id, key.exchange_id, e.entry_type, e.base_currency, real(&e.base_amount), e.quote_currency, e.quote_amount.as_ref().map(real),
                    if no_fee { None } else { e.fee_currency.as_deref() }, if no_fee { None } else { e.fee_amount.as_ref().map(real) }, tx_id, e.group_id,
                    e.description, sql_time(at), e.raw.text(), asset_id(c, &p.listings, Some(base_currency), &e.raw.value)?, bot_order, sql_time(now)])?;
        p.merged.extend(merged_ids(&e.raw.value));
        if split(e) {
            let effects = effects.ok_or_else(no_effects)?;
            p.out.splits.push(apply_split(c, base_currency, at, e.raw.value["split_ratio"].as_str(), effects, now)?);
            if at > now {
                let mut owed = load_owed(c, key.id)?;
                owed.push(Owed { symbol: base_currency.to_string(), exchange_id: key.exchange_id, at: at.timestamp_micros() });
                save_owed(c, key.id, &owed, now)?;
            }
        }
        p.out.imported += 1;
    }
    Ok(p)
}

fn merged_ids(raw: &Value) -> Vec<String> {
    raw.get("merged_activity_ids").and_then(Value::as_array).map(|ids| ids.iter().filter_map(|i| i.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// The index on `transacted_at` (db/schema.rb). The two lookups by time name it, because SQLite's planner otherwise
/// takes the index on `user_id` and walks the user's whole history for each row, inside the write transaction.
const BY_TIME: &str = "INDEXED BY index_account_transactions_on_transacted_at";

/// #duplicate?, scoped to the user and the venue, not the key. With an id: a leg of a stored merged split, a stored
/// row with that id, or an id-less stored row of the same type, currency and amount less than a second away (a file
/// imported before the first sync). Without one: any stored row of that identity less than a second away.
fn duplicate(c: &Connection, key: &Key, e: &Entry, tx_id: Option<&str>, merged: &HashSet<String>) -> Result<bool, SyncError> {
    if let Some(tx_id) = tx_id {
        if merged.contains(tx_id) { return Ok(true); }
        let stored: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND tx_id = ?3)",
                                       params![key.user_id, key.exchange_id, tx_id], |r| r.get(0))?;
        if stored { return Ok(true); }
    }
    let Some(at) = e.transacted_at else { return Ok(false) }; // a row with no time is the same event as nothing
    let second = Duration::seconds(1);
    let (Some(from), Some(to)) = (at.checked_sub_signed(second), at.checked_add_signed(second)) else { return Err(SyncError("a time at the end of the calendar".into())) };
    let id_less = if tx_id.is_some() { "AND tx_id IS NULL" } else { "" };
    Ok(c.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM account_transactions {BY_TIME} WHERE user_id = ?1 AND exchange_id = ?2 {id_less} AND entry_type = ?3 \
                  AND base_currency IS ?4 AND base_amount = ?5 AND transacted_at > ?6 AND transacted_at < ?7)"),
        params![key.user_id, key.exchange_id, e.entry_type, e.base_currency, real(&e.base_amount), sql_time(from), sql_time(to)],
        |r| r.get(0))?)
}

/// #match_bot_transaction!: the bot order this row is, by the row's own id, then by the order the fill names.
fn bot_order(c: &Connection, exchange_id: i64, tx_id: Option<&str>, raw: &Value) -> Result<Option<i64>, SyncError> {
    for id in [tx_id, raw["order_id"].as_str().filter(|s| !blank(s))].into_iter().flatten() {
        let found: Option<i64> = c.query_row("SELECT id FROM transactions WHERE external_id = ?1 AND exchange_id = ?2 LIMIT 1", params![id, exchange_id], |r| r.get(0)).optional()?;
        if found.is_some() { return Ok(found); }
    }
    Ok(None)
}

/// Ticker.asset_ids_named: every asset the venue lists under this name, by a base spelling or by the asset's symbol.
fn asset_ids_named(c: &Connection, exchange_id: i64, name: &str) -> Result<Vec<i64>, SyncError> {
    let wanted = name.to_uppercase();
    let mut ids: Vec<i64> = vec![];
    let mut s = c.prepare("SELECT t.base, t.base_asset_id, upper(a.symbol) FROM tickers t JOIN assets a ON a.id = t.base_asset_id WHERE t.exchange_id = ?1 ORDER BY t.id")?;
    let rows = s.query_map([exchange_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<String>>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
    for (base, asset_id, symbol) in rows {
        if (spelling(&base) == wanted || symbol.as_deref() == Some(wanted.as_str())) && !ids.contains(&asset_id) { ids.push(asset_id); }
    }
    Ok(ids)
}

/// AccountTransactionSync.rows_naming as a WHERE clause over `transactions`: the user's submitted orders on this
/// venue for the asset a symbol names (params: ?1 exchange, ?2 user, ?3 symbol).
fn rows_naming(c: &Connection, exchange_id: i64, symbol: &str) -> Result<String, SyncError> {
    let ids = asset_ids_named(c, exchange_id, symbol)?.iter().map(i64::to_string).collect::<Vec<_>>().join(", ");
    let by_asset = if ids.is_empty() { String::new() } else { format!(" OR base_asset_id IN ({ids})") };
    Ok(format!("status = 0 AND exchange_id = ?1 AND bot_id IN (SELECT id FROM bots WHERE user_id = ?2) AND (base = ?3{by_asset})"))
}

/// The bots AccountTransactionSync.expire_restated_bots moves: every bot of the user that traded the symbol on this venue.
fn bots_naming(c: &Connection, user_id: i64, exchange_id: i64, symbol: &str) -> Result<Vec<i64>, SyncError> {
    if blank(symbol) { return Ok(vec![]); }
    let naming = rows_naming(c, exchange_id, symbol)?;
    let mut s = c.prepare(&format!("SELECT id FROM bots WHERE id IN (SELECT bot_id FROM transactions WHERE {naming}) ORDER BY id"))?;
    let bots = s.query_map(params![exchange_id, user_id, symbol], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(bots)
}

/// AccountTransactionSync.expire_restated_bots: every bot of the user that traded the symbol on this venue gets its
/// `restatement_generation` moved by one (Rails' cache generation for that bot's metrics). One statement, one column.
/// Also what runs at the effective time of a split that was imported ahead of its date (Bot::ExpireRestatedMetricsJob).
/// The scan is a read unit; the bump is one write unit past the engine's guard (`commit_bots`), so a refusal rolls back
/// every counter of it. The bare bump is private: no caller writes `bots` outside a guarded transaction.
pub async fn expire_restated(db: &Db, user_id: i64, exchange_id: i64, symbol: &str) -> Result<Vec<i64>, SyncError> {
    let symbol = symbol.to_string();
    let bots = Arc::new(phase(db, move |c| bots_naming(c, user_id, exchange_id, &symbol)).await?);
    let named = bots.clone();
    commit_bots(db, move |c| { bump(c, &named)?; Ok(((), !named.is_empty())) }).await?;
    Ok(bots.to_vec())
}

fn bump(c: &Connection, bots: &[i64]) -> Result<(), SyncError> {
    for id in bots {
        c.execute("UPDATE bots SET restatement_generation = COALESCE(restatement_generation, 0) + 1 WHERE id = ?1", [id])?;
    }
    Ok(())
}

/// What a split row does to the user's bots, worked out by a read before the row's own write transaction: the scans of
/// the bots' orders happen outside the write lock, and the write is one statement per bot named here.
#[derive(Clone, Debug, Default)]
struct Effects {
    /// AccountTransactionSync.expire_restated_bots' bots.
    restated: Vec<i64>,
    /// AccountTransactionSync.announce_split's bots; none for a split older than the feed keeps.
    holding: Vec<i64>,
}

fn split_effects(c: &Connection, key: &Key, e: &Entry, now: DateTime<Utc>) -> Result<Effects, SyncError> {
    let (Some(symbol), Some(at)) = (present(&e.base_currency), e.transacted_at) else { return Ok(Effects::default()) };
    let holding = if at < now - Duration::days(FEED_RETENTION_DAYS) { vec![] } else { bots_holding(c, key, symbol, at)? };
    Ok(Effects { restated: bots_naming(c, key.user_id, key.exchange_id, symbol)?, holding })
}

/// AccountTransactionSync#log_split for a split row just stored: the counters, then one `asset_split`
/// line, dated at the split, on every bot that was holding the symbol (AccountTransactionSync.announce_split); a line
/// already there for that instant and symbol is upgraded instead (a leg first, the ratio later).
fn apply_split(c: &Connection, symbol: &str, at: DateTime<Utc>, ratio: Option<&str>, effects: &Effects, now: DateTime<Utc>) -> Result<Split, SyncError> {
    bump(c, &effects.restated)?;
    let mut details = json!({ "base": symbol });
    if let Some(r) = ratio { details["ratio"] = json!(r); }
    for bot_id in &effects.holding {
        let mut s = c.prepare("SELECT id, details FROM bot_activity_logs WHERE bot_id = ?1 AND event = 'asset_split' AND created_at = ?2 ORDER BY id")?;
        let lines = s.query_map(params![bot_id, sql_time(at)], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
        let existing = lines.into_iter().find(|(_, d)| serde_json::from_str::<Value>(d).is_ok_and(|d| d["base"] == symbol));
        match existing {
            Some((id, _)) => { c.execute("UPDATE bot_activity_logs SET details = ?1 WHERE id = ?2", params![details.to_string(), id])?; }
            None => { c.execute("INSERT INTO bot_activity_logs (bot_id, event, level, details, created_at) VALUES (?1, 'asset_split', 0, ?2, ?3)",
                                params![bot_id, details.to_string(), sql_time(at)])?; }
        }
    }
    Ok(Split { symbol: symbol.to_string(), at, restated_bots: effects.restated.clone(), effective_later: at > now })
}

/// AccountTransactionSync.bots_holding: the bots that still had the symbol at the split, by what their orders before
/// it actually moved (Transaction.confirmed_exec_amounts). A position that reads exactly flat is left out, unless
/// the bot's orders reach back past an earlier split of the symbol (then the units disagree and a zero proves nothing).
fn bots_holding(c: &Connection, key: &Key, symbol: &str, at: DateTime<Utc>) -> Result<Vec<i64>, SyncError> {
    let naming = rows_naming(c, key.exchange_id, symbol)?;
    let mut s = c.prepare(&format!("SELECT bot_id, side, external_status, amount, amount_exec, created_at FROM transactions WHERE {naming} AND created_at < ?4"))?;
    let mut q = s.query(params![key.exchange_id, key.user_id, symbol, sql_time(at)])?;
    let mut net: Vec<(i64, BigDec, String)> = vec![]; // bot, net position, earliest order
    while let Some(r) = q.next()? {
        let (bot_id, side, status): (i64, Option<i64>, Option<i64>) = (r.get(0)?, r.get(1)?, r.get(2)?);
        let (amount, exec) = (number::stored(r.get_ref(3)?).map_err(SyncError)?, number::stored(r.get_ref(4)?).map_err(SyncError)?);
        let created_at: String = r.get(5)?;
        // A closed order with no recorded fill moved what it asked for.
        let Some(exec) = exec.or(if status == Some(2) { amount } else { None }).filter(|e| !e.is_zero()) else { continue };
        let signed = if side == Some(1) { &BigDec::zero() - &exec } else { exec };
        match net.iter_mut().find(|n| n.0 == bot_id) {
            Some(n) => { n.1 = &n.1 + &signed; if created_at < n.2 { n.2 = created_at; } }
            None => net.push((bot_id, signed, created_at)),
        }
    }
    // #last_split_before: when this symbol was last restated on this venue before `at`.
    let mut s = c.prepare("SELECT transacted_at, raw_data FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND entry_type = ?3 AND base_currency = ?4 AND transacted_at < ?5")?;
    let rows = s.query_map(params![key.user_id, key.exchange_id, ADJUSTMENT, symbol, sql_time(at)], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let restated_at = rows.into_iter().filter(|(_, raw)| {
        let raw: Value = raw.as_deref().and_then(|r| serde_json::from_str(r).ok()).unwrap_or(Value::Null);
        raw["corporate_action"] == "split" || raw["activity_type"].as_str().is_some_and(|t| activities::SPLIT_TYPES.contains(&t))
    }).map(|(t, _)| t).max();
    let mut holding: Vec<i64> = net.into_iter()
        .filter(|(_, amount, opened)| !(amount.is_zero() && restated_at.as_ref().is_none_or(|r| opened >= r)))
        .map(|(bot, _, _)| bot).collect();
    holding.sort_unstable();
    Ok(holding)
}

/// One page of #resolve_recent_assets' candidates.
struct AssetPage { found: Vec<(i64, i64)>, next: Option<i64> }

/// #resolve_recent_assets, read side: of at most `BATCH` rows stored in the last seven days with no asset (in id order,
/// after `after_id`), those the venue's listings now name an asset for, as (asset, row). `next`: where the next page starts.
fn unresolved_assets(c: &Connection, key: &Key, l: &Listings, now: DateTime<Utc>, after_id: i64) -> Result<AssetPage, SyncError> {
    let mut s = c.prepare("SELECT id, base_currency, raw_data FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND created_at >= ?3 AND base_asset_id IS NULL \
                           AND id > ?4 ORDER BY id LIMIT ?5")?;
    let rows = s.query_map(params![key.user_id, key.exchange_id, sql_time(now - Duration::days(ASSET_CATCH_UP_DAYS)), after_id, BATCH as i64],
                           |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
    let next = if rows.len() == BATCH { rows.last().map(|r| r.0) } else { None };
    let mut found = vec![];
    for (id, base_currency, raw) in rows {
        let raw: Value = raw.as_deref().and_then(|r| serde_json::from_str(r).ok()).unwrap_or(Value::Null);
        if let Some(asset) = asset_id(c, l, base_currency.as_deref(), &raw)? { found.push((asset, id)); }
    }
    Ok(AssetPage { found, next })
}

/// #resolve_recent_assets, write side: a recorded asset is never replaced.
fn set_assets(c: &Connection, found: &[(i64, i64)]) -> Result<(), SyncError> {
    for (asset, id) in found {
        c.execute("UPDATE account_transactions SET base_asset_id = ?1 WHERE id = ?2 AND base_asset_id IS NULL", params![asset, id])?;
    }
    Ok(())
}

/// One page of TransferMatcher's work: (deposit, withdrawal) pairs to link, and where the next page starts.
struct TransferPage { pairs: Vec<(i64, i64)>, next: Option<i64> }

/// TransferMatcher.run!, read side, for at most `BATCH` of the user's unlinked, non-rejected withdrawals (in id order,
/// after `after_id`): each is paired with the earliest unclaimed deposit of the same currency within 72 hours after it
/// whose amount is 0 to 2 % below it. Any venue. "Unclaimed" is a keyed lookup on the unique index of
/// `linked_transaction_id`, plus the deposits this page has already paired (at most `BATCH`).
fn transfer_matches(c: &Connection, user_id: i64, after_id: i64) -> Result<TransferPage, SyncError> {
    let mut s = c.prepare("SELECT id, base_currency, base_amount, transacted_at FROM account_transactions \
                           WHERE user_id = ?1 AND entry_type = 5 AND linked_transaction_id IS NULL AND transfer_link_rejected = 0 AND id > ?2 ORDER BY id LIMIT ?3")?;
    let mut withdrawals = vec![];
    let mut q = s.query(params![user_id, after_id, BATCH as i64])?;
    while let Some(r) = q.next()? {
        withdrawals.push((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, number::stored(r.get_ref(2)?).map_err(SyncError)?.unwrap_or_else(BigDec::zero), parse_time(&r.get::<_, String>(3)?)?));
    }
    let next = if withdrawals.len() == BATCH { withdrawals.last().map(|w| w.0) } else { None };
    let tolerance = BigDec::parse("0.98").map_err(SyncError::from)?;
    let mut deposits = c.prepare(&format!(
        "SELECT d.id FROM account_transactions d {BY_TIME} WHERE d.user_id = ?1 AND d.entry_type = 4 AND d.base_currency = ?2 AND d.transacted_at BETWEEN ?3 AND ?4 \
         AND d.base_amount BETWEEN ?5 AND ?6 AND NOT EXISTS (SELECT 1 FROM account_transactions l WHERE l.linked_transaction_id = d.id AND l.user_id = ?1) \
         ORDER BY d.transacted_at ASC, d.id ASC LIMIT ?7"))?;
    let mut pairs: Vec<(i64, i64)> = vec![];
    for (id, currency, amount, at) in withdrawals {
        // A withdrawal with no time 72 hours after it (a stored time at the end of the calendar) has no window.
        let Some(until) = at.checked_add_signed(Duration::hours(TRANSFER_WINDOW_HOURS)) else { continue };
        let candidates = deposits.query_map(
            params![user_id, currency, sql_time(at), sql_time(until), real(&(&amount * &tolerance)), real(&amount), pairs.len() as i64 + 1],
            |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
        if let Some(deposit) = candidates.into_iter().find(|d| !pairs.iter().any(|(taken, _)| taken == d)) { pairs.push((deposit, id)); }
    }
    Ok(TransferPage { pairs, next })
}

/// TransferMatcher.run!, write side: a link is written only if the withdrawal is still unlinked and the deposit still unclaimed.
fn link(c: &Connection, pairs: &[(i64, i64)], now: DateTime<Utc>) -> Result<usize, SyncError> {
    let mut linked = 0;
    for (deposit, withdrawal) in pairs {
        linked += c.execute("UPDATE account_transactions SET linked_transaction_id = ?1, updated_at = ?2 WHERE id = ?3 AND linked_transaction_id IS NULL \
                             AND transfer_link_rejected = 0 AND NOT EXISTS (SELECT 1 FROM account_transactions l WHERE l.linked_transaction_id = ?1)",
                            params![deposit, sql_time(now), withdrawal])?;
    }
    Ok(linked)
}

/// TransferMatcher.run! for one user, a page of withdrawals at a time: a read that pairs, then one write transaction
/// for that page's links.
pub async fn link_transfers(db: &Db, user_id: i64, now: DateTime<Utc>) -> Result<usize, SyncError> {
    let (mut after_id, mut linked) = (0, 0);
    loop {
        let page = phase(db, move |c| transfer_matches(c, user_id, after_id)).await?;
        if !page.pairs.is_empty() { linked += commit(db, move |c| link(c, &page.pairs, now)).await?; }
        match page.next { Some(id) => after_id = id, None => return Ok(linked) }
    }
}
