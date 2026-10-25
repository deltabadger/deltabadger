//! Kraken's syncs (plan Kraken sync). `KrakenIdentity`: which asset a Kraken asset code names (§2.4, D-KR-2, D-KR-3), as
//! the balance sync reads BalanceEx and the ledger reads Ledgers. The ledger sync (K2, K3): Exchanges::Kraken#get_ledger
//! then AccountTransactionSync#store! and TransferMatcher.run!, on the continuation kit.
//!
//! As Rails reads it: a throttled page (`EAPI:Rate limit exceeded`, and HTTP 429/418) is asked again up to 6 times, 10 s
//! apart; any other failure fails the sync with the venue's first error. 25 h overlap, the ledger id as tx_id, the refid as
//! group_id, the watermark from the data, no bot linking.
//!
//! Where it differs on purpose (listed in rust/tests/sync_parity.rs, `listed_kraken`):
//! - D-KR-W: cursor paging (`with_cursor`, 50 a page) in two walks, the currency ledger then `aclass=tokenized_asset`,
//!   each with its own cursor in the import's record; a long import goes on over several runs (sync::continuation).
//! - D-KR-F: one row per ledger id at Kraken's own balance change, `amount − fee` (`normalise`): trade legs keep their fee
//!   (the walk takes it exactly), every other row folds it; a negative fee is always folded.
//! - D-KR-3: codes and assets by `KrakenIdentity` (`AssetRule::Kraken`): the asset's own symbol is written.
//! - D-KR-4: every ledger id is stored; internal pairs (earn allocations, spot ↔ staking) are linked by refid
//!   (`link_internal`) and kept out of the generic transfer matcher.
//! - D-KR-5: rows an earlier sync wrote are rewritten to what this sync reads (`repair`), by the conflict rules of plan §2.6
//!   and r3 rows 2 and 5: a linked row is written only into a valid pair, else deferred to the import's completion.
use super::activities::{Entry, Raw, AIRDROP, BUY, DEPOSIT, FEE, OTHER_INCOME, SELL, UNSUPPORTED_ACTIVITY, WITHDRAWAL};
use super::binance_ledger::{page_of, ruby_time_at_secs_f, Fail};
use super::continuation;
use super::ledger::{only, rule_venue, spellings, AssetRule, Outcome};
use super::wire::Node;
use super::{capture_bound_version, load_key_of, phase, sql_time, Failure, Key, SyncError};
use crate::crypto::Credentials;
use crate::engine::Clock;
use crate::jobs::Db;
use crate::ruby::BigDec;
use crate::venue::http::Transport;
use crate::venue::kraken::{KrakenVenue, LEDGERS};
use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub const KRAKEN: &str = "Exchanges::Kraken";

/// Kraken's legacy X-codes that Exchanges::Kraken::ASSET_MAP lacks (D-KR-2): Rails drops a balance under any of them.
const LEGACY: [(&str, &str); 8] = [("XLTC", "LTC"), ("XXRP", "XRP"), ("XXLM", "XLM"), ("XXMR", "XMR"), ("XETC", "ETC"), ("XZEC", "ZEC"),
                                   ("XMLN", "MLN"), ("XREP", "REP")];
/// The terminal suffixes of a code that holds the same asset elsewhere in the account: staked (.S), opt-in rewards (.M),
/// parachain (.P), yield-bearing (.B), Kraken Rewards flexible (.F). One is stripped, never more.
// ponytail: `.P` is not in Kraken's docs, only seen in the wild; drop it if a live sample shows another meaning.
const SUFFIXES: [&str; 5] = [".S", ".M", ".P", ".B", ".F"];

/// Which asset a Kraken code names, over every listing of the venue (unavailable and tombstoned included, D-KR-6). Each
/// step answers only when exactly one asset fits; two candidates or none fall to the next step, and after the last to None.
/// 1. The code itself (Tax::AssetIdentity's dated alias first, then the listings' spellings).
/// 2. ASSET_MAP and the legacy X-codes, then step 1.
/// 3. One terminal suffix stripped, then steps 1–2.
/// 4. A `.T` code: the tokenized resolver only, never the coin (D-KR-3).
/// 5. The tokenized resolver on the code (NVDASPV is NVDAx).
/// 6. An asset listed on Kraken whose symbol is the code (the canonical symbol a CSV row carries, r3).
pub struct KrakenIdentity {
    venue: crate::tracker::prices::Venue,
    spellings: HashMap<String, Vec<i64>>,
    /// The assets marked as tokenized wrappers (Asset.mark_tokenized!).
    tokenized: HashSet<i64>,
    /// The listed assets by their own symbol.
    symbols: HashMap<String, Vec<i64>>,
    /// The aliases' coins by external id.
    aliases: HashMap<String, i64>,
    /// Each listed (or alias) asset's own symbol: what a ledger row writes.
    names: HashMap<i64, String>,
}

impl KrakenIdentity {
    pub fn load(c: &Connection, exchange_id: i64) -> Result<Self, SyncError> {
        let listed = "SELECT DISTINCT a.id, a.symbol, a.instrument_type FROM assets a JOIN tickers t ON a.id IN (t.base_asset_id, t.quote_asset_id) WHERE t.exchange_id = ?1 ORDER BY a.id";
        let mut s = c.prepare(listed)?;
        let rows = s.query_map([exchange_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
        let (mut tokenized, mut symbols, mut names) = (HashSet::new(), HashMap::<String, Vec<i64>>::new(), HashMap::new());
        for (id, symbol, kind) in rows {
            if kind.as_deref() == Some("tokenized") { tokenized.insert(id); }
            if let Some(symbol) = symbol { symbols.entry(symbol.clone()).or_default().push(id); names.insert(id, symbol); }
        }
        let mut aliases = HashMap::new();
        let mut s = c.prepare("SELECT id, symbol FROM assets WHERE external_id = ?1 LIMIT 1")?;
        for coin in crate::tracker::prices::alias_coins() {
            if let Some((id, symbol)) = s.query_row([coin], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))).optional()? {
                aliases.insert(coin.to_string(), id);
                if let Some(symbol) = symbol { names.entry(id).or_insert(symbol); }
            }
        }
        Ok(Self { venue: rule_venue(exchange_id, KRAKEN), spellings: spellings(c, exchange_id)?, tokenized, symbols, aliases, names })
    }

    /// The asset `code` names now, if exactly one does.
    pub fn asset_id(&self, code: &str) -> Option<i64> { self.asset_id_on(code, None) }

    /// The asset `code` names on `on` (a dated alias answers only up to its date; None: now, when every dated alias has lapsed).
    pub fn asset_id_on(&self, code: &str, on: Option<NaiveDate>) -> Option<i64> {
        if let Some(head) = code.strip_suffix(".T") { return self.tokenized(head); }
        self.mapped(code, on)
            .or_else(|| SUFFIXES.iter().find_map(|s| code.strip_suffix(s)).and_then(|head| self.mapped(head, on)))
            .or_else(|| self.tokenized(code))
            .or_else(|| only(self.symbols.get(code)))
    }

    /// What a ledger row of `code` records: the asset's own symbol and id, else the mapped code (ASSET_MAP and the legacy
    /// X-codes, XBT as BTC) and no asset.
    pub fn identify(&self, code: &str, on: Option<NaiveDate>) -> (String, Option<i64>) {
        match self.asset_id_on(code, on) {
            Some(id) => (self.names.get(&id).cloned().unwrap_or_else(|| mapped_code(code)), Some(id)),
            None => (mapped_code(code), None),
        }
    }

    fn mapped(&self, code: &str, on: Option<NaiveDate>) -> Option<i64> {
        let map = crate::venue::kraken::ASSET_MAP.iter().chain(LEGACY.iter()).find(|(k, _)| *k == code).map(|(_, v)| *v);
        self.plain(code, on).or_else(|| map.and_then(|name| self.plain(name, on)))
    }

    /// AssetRule::Crypto's answer for one name: a dated alias names its coin, else the one listing spelt so.
    fn plain(&self, name: &str, on: Option<NaiveDate>) -> Option<i64> {
        if let Some(coin) = crate::tracker::prices::alias_coin(name, Some(&self.venue), on) { return self.aliases.get(&coin).copied(); }
        only(self.spellings.get(&name.to_uppercase()))
    }

    /// Exchanges::Kraken#tokenized_asset_from_symbol: the SPV code spelt as its x form, upcased, among tokenized wrappers only.
    fn tokenized(&self, code: &str) -> Option<i64> {
        let name = code.strip_suffix("SPV").map_or_else(|| code.to_string(), |head| format!("{head}x")).to_uppercase();
        let ids: Vec<i64> = self.spellings.get(&name)?.iter().copied().filter(|id| self.tokenized.contains(id)).collect();
        only(Some(&ids))
    }
}

/// A code no asset is named by, as Rails spells it: ASSET_MAP (and the legacy X-codes), XBT as BTC.
fn mapped_code(code: &str) -> String {
    let mapped = crate::venue::kraken::ASSET_MAP.iter().chain(LEGACY.iter()).find(|(k, _)| *k == code).map_or(code, |(_, v)| *v);
    if mapped == "XBT" { "BTC".into() } else { mapped.into() }
}

// ---- the ledger walk (D-KR-W) ----------------------------------------------------------------------------------------

/// Exchanges::Kraken::LEDGER_THROTTLE_RETRIES and LEDGER_THROTTLE_WAIT.
const THROTTLE_RETRIES: usize = 6;
pub const THROTTLE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);
const THROTTLE: &str = "EAPI:Rate limit exceeded";
pub const OVERLAP_HOURS: i64 = 25;
/// Kraken's cursor page.
const PAGE_ROWS: usize = 50;
/// ponytail: Kraken documents no page cap; one stops a cursor that never ends.
const MAX_PAGES: u64 = 100_000;
pub const MAX_PAGE_BYTES: usize = 1024 * 1024;
pub const MAX_PAGE_NODES: usize = 50_000;

/// Which walk of the ledger the import is in: the currency ledger (Kraken's default `aclass`), then the tokenized assets.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Source { Currency, Tokenized, Done }

impl Source {
    fn name(self) -> &'static str { match self { Self::Currency => "currency", Self::Tokenized => "tokenized", Self::Done => "done" } }
    fn named(s: &str) -> Option<Self> { [Self::Currency, Self::Tokenized, Self::Done].into_iter().find(|v| v.name() == s) }
}

/// A page that could not be read: why, and whether it is the venue's own `error` (a resumed cursor it rejects).
struct PageFail { fail: Fail, venue: bool }
impl From<Fail> for PageFail { fn from(fail: Fail) -> Self { Self { fail, venue: false } } }

/// One page's ledger entries (id, entry) and the cursor to the next, if any.
type Page = (Vec<(String, Raw)>, Option<String>);

/// One answer as Exchanges::Kraken#get_ledger reads it: honeymaker's failure or a non-empty `error` fails it (its first
/// entry); `result.ledger` may be absent, null or empty (no rows); `result.cursor.next` names the next page.
fn read_page(body: &str) -> Result<Page, PageFail> {
    let node = page_of(body, "Kraken ledger", MAX_PAGE_NODES)?;
    let raw = Raw::from_node(&node).map_err(|_| ("unreadable Kraken ledger".to_string(), true))?;
    if let Some(Value::Array(errors)) = raw.value.get("error") {
        if let Some(first) = errors.first() {
            return Err(PageFail { fail: (first.as_str().map_or_else(|| first.to_string(), str::to_string), false), venue: true });
        }
    }
    let Some(result) = raw.object("result").map_err(|why| (format!("Kraken ledger: {why}"), true))? else { return Ok((vec![], None)) };
    let mut rows = vec![];
    match result.member("ledger") {
        None => {}
        Some(Node::Scalar(t)) if t == "null" => {}
        Some(Node::Array(items)) if items.is_empty() => {}
        Some(Node::Object(entries)) => for (id, entry) in entries {
            rows.push((id.clone(), Raw::from_node(entry).map_err(|_| ("unreadable Kraken ledger entry".to_string(), true))?));
        },
        Some(_) => return Err(("unreadable Kraken ledger".to_string(), true).into()),
    }
    let next = result.value["cursor"]["next"].as_str().filter(|n| !n.is_empty()).map(str::to_string);
    Ok((rows, next))
}

/// The internal moves between a Kraken account's own wallets (spot, staking, earn): `type/subtype`.
pub const INTERNAL: [&str; 8] = ["earn/allocation", "earn/deallocation", "earn/autoallocate", "earn/migration",
                                 "transfer/spottostaking", "transfer/stakingfromspot", "transfer/stakingtospot", "transfer/spotfromstaking"];

/// A Kraken row that is one leg of an internal move (`alias`: the table's alias with its dot, or ""): linked by refid
/// (`link_internal`), never by the generic matcher.
pub(super) fn internal_leg_sql(alias: &str) -> String {
    let list = INTERNAL.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(", ");
    format!("({alias}exchange_id IN (SELECT id FROM exchanges WHERE type = '{KRAKEN}') AND json_valid({alias}raw_data) \
             AND COALESCE(json_extract({alias}raw_data, '$.type') || '/' || json_extract({alias}raw_data, '$.subtype') IN ({list}), 0))")
}

const TRADE_GROUP: [&str; 5] = ["trade", "spend", "receive", "adjustment", "conversion"];
const STAKING: i64 = 6;

/// Plan §2.3 as amended by r3 row 1: one row per ledger id, at Kraken's balance change `delta = amount − fee`, the sign
/// deciding. Trade-group legs keep a positive fee beside them (the walk takes it exactly); a negative fee, and every other
/// type's fee, is folded into the amount. The type names the row; a zero change is `unsupported_activity`.
pub(super) fn normalise(id: &str, raw: Raw, identity: &KrakenIdentity) -> Result<Entry, Fail> {
    let value = |k: &str| raw.value[k].as_str().unwrap_or_default().to_string();
    let (kind, subtype) = (value("type"), value("subtype"));
    let code = raw.value["asset"].as_str().ok_or_else(|| ("unreadable ledger entry asset".to_string(), true))?.to_string();
    let amount = raw.number("amount").map_err(|why| (format!("ledger entry: {why}"), true))?.ok_or_else(|| ("unreadable ledger entry amount: missing".to_string(), true))?;
    let fee = raw.number("fee").map_err(|why| (format!("ledger entry: {why}"), true))?.unwrap_or_else(BigDec::zero);
    let at = raw.value["time"].as_f64().and_then(ruby_time_at_secs_f).ok_or_else(|| ("unreadable ledger entry time".to_string(), true))?;
    let symbol = identity.identify(&code, Some(at.date_naive())).0;
    let zero = BigDec::zero();
    let abs = |d: &BigDec| if *d < zero { &zero - d } else { d.clone() };
    let delta = &amount - &fee;
    let folded = |positive: i64, negative: i64| -> (i64, BigDec, bool) {
        if delta.is_zero() { (UNSUPPORTED_ACTIVITY, zero.clone(), false) } else if delta.is_positive() { (positive, delta.clone(), false) } else { (negative, abs(&delta), false) }
    };
    let trade = TRADE_GROUP.contains(&kind.as_str());
    let (entry_type, base, keep_fee) = if trade {
        if fee < zero { folded(BUY, SELL) }
        else if amount.is_positive() && fee <= amount { (BUY, amount.clone(), true) }
        else if amount < zero { (SELL, abs(&amount), true) }
        else if fee > amount { (SELL, &fee - &amount, false) }
        else { (UNSUPPORTED_ACTIVITY, zero.clone(), false) }
    } else {
        let pair = format!("{kind}/{subtype}");
        match kind.as_str() {
            "staking" | "reward" => folded(STAKING, WITHDRAWAL),
            "earn" if subtype == "reward" => folded(STAKING, WITHDRAWAL),
            "dividend" => folded(OTHER_INCOME, WITHDRAWAL),
            "rollover" | "creator_fee" | "nftcreatorfee" => folded(DEPOSIT, FEE),
            "transfer" if !INTERNAL.contains(&pair.as_str()) && !["spottofutures", "spotfromfutures"].contains(&subtype.as_str()) => folded(AIRDROP, WITHDRAWAL),
            _ => folded(DEPOSIT, WITHDRAWAL),
        }
    };
    Ok(Entry {
        entry_type, base_currency: Some(symbol.clone()), base_amount: base, quote_currency: None, quote_amount: None,
        fee_currency: keep_fee.then(|| symbol.clone()), fee_amount: keep_fee.then_some(fee),
        tx_id: Some(id.to_string()), group_id: raw.value["refid"].as_str().map(str::to_string),
        description: (!trade).then(|| if subtype.is_empty() { format!("Kraken {kind}") } else { format!("Kraken {kind}/{subtype}") }),
        transacted_at: Some(at), raw,
    })
}

/// Exchanges::Kraken#get_ledger from `start` (None: the whole history), a cursor page at a time: the currency ledger, then
/// the tokenized one (`aclass=tokenized_asset`), each source with its own cursor.
struct Walk<'a, T: Transport> {
    venue: &'a KrakenVenue<T>, identity: &'a KrakenIdentity, wait: std::time::Duration, start: Option<i64>,
    source: Source, cursor: Option<String>, pages: u64,
    /// The cursor came from the record (a resumed run) and has not been answered yet; a rejection restarts the source once.
    resumed: bool, restarted: bool,
}

impl<'a, T: Transport> Walk<'a, T> {
    fn new(venue: &'a KrakenVenue<T>, identity: &'a KrakenIdentity, wait: std::time::Duration, begin: continuation::Begin<'_>) -> Self {
        let mut walk = Self { venue, identity, wait, start: begin.start.map(|s| s.timestamp()), source: Source::Currency, cursor: None, pages: 0, resumed: false, restarted: false };
        if let Some((c, source)) = begin.cursor.and_then(|c| Some((c, Source::named(c["source"].as_str()?)?))) {
            walk.source = source;
            walk.cursor = c["cursor"].as_str().map(str::to_string);
            walk.pages = c["pages"].as_u64().unwrap_or_default();
            walk.resumed = walk.cursor.is_some();
        }
        walk
    }
    fn fields(&self) -> Vec<(&'static str, String)> {
        let mut fields: Vec<(&'static str, String)> = self.start.map(|s| ("start", s.to_string())).into_iter().collect();
        fields.push(("with_cursor", "true".into()));
        if let Some(c) = &self.cursor { fields.push(("cursor", c.clone())); }
        // ponytail: the deprecated `aclass` form field, as honeymaker's wire is a form body; switch to a JSON body and the
        // `{asset, aclass}` list form if Kraken removes `aclass`.
        if self.source == Source::Tokenized { fields.push(("aclass", "tokenized_asset".into())); }
        fields
    }
}

impl<T: Transport> continuation::Walk for Walk<'_, T> {
    fn done(&self) -> bool { self.source == Source::Done }
    fn cursor(&self) -> Value { json!({ "source": self.source.name(), "cursor": self.cursor, "pages": self.pages }) }
    fn page_rows(&self) -> usize { PAGE_ROWS }
    fn request_reserve(&self) -> std::time::Duration { crate::venue::http::TOTAL_TIMEOUT * (1 + THROTTLE_RETRIES as u32) + self.wait * THROTTLE_RETRIES as u32 }
    const KRAKEN_REPAIR: bool = true;
    /// At the import's completion (its done cursor), the repairs deferred on its pages converge (r3 row 5).
    const CHECKPOINT: continuation::Checkpoint = |c, key, cursor, now| if cursor["source"] == "done" { converge(c, key, now) } else { Ok(()) };
    async fn step(&mut self) -> Result<Vec<Entry>, Fail> {
        let mut retried = 0usize;
        let (rows, next) = loop {
            let read = match self.venue.read_form(LEDGERS, &self.fields(), MAX_PAGE_BYTES, true).await {
                Err(text) => Err(PageFail { fail: (text, false), venue: false }),
                Ok(body) => read_page(&body),
            };
            match read {
                Ok(page) => break page,
                Err(PageFail { fail: (text, false), .. }) if text.contains(THROTTLE) && retried < THROTTLE_RETRIES => {
                    retried += 1;
                    if !self.wait.is_zero() { tokio::time::sleep(self.wait).await; }
                }
                // ponytail: a cursor's lifetime is undocumented. One the venue refuses after a pause between runs restarts
                // its source from `start`, once; what the restart reads again is a duplicate.
                Err(PageFail { venue: true, .. }) if self.resumed && !self.restarted => {
                    (self.cursor, self.resumed, self.restarted) = (None, false, true);
                    return Ok(vec![]);
                }
                Err(f) => return Err(f.fail),
            }
        };
        self.resumed = false;
        let mut out = Vec::with_capacity(rows.len());
        for (id, raw) in rows { out.push(normalise(&id, raw, self.identity)?); }
        self.pages += 1;
        if self.pages > MAX_PAGES { return Err((format!("Too many pages of the Kraken {} ledger", self.source.name()), false)); }
        match next {
            Some(n) if Some(&n) == self.cursor.as_ref() => return Err((format!("Kraken {} ledger paging is stuck: the cursor did not move", self.source.name()), false)),
            Some(n) => self.cursor = Some(n),
            None => {
                self.source = if self.source == Source::Currency { Source::Tokenized } else { Source::Done };
                (self.cursor, self.pages, self.restarted) = (None, 0, false);
            }
        }
        Ok(out)
    }
}

/// One ledger sync of one Kraken key, held to the job's deadline (sync::continuation). `wait` is the throttle wait
/// (THROTTLE_WAIT; the parity harness passes zero, as Rails' harness stubs its sleep).
pub async fn sync<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock, wait: std::time::Duration) -> Result<Result<Outcome, Failure>, SyncError> {
    let stopwatch = continuation::Wall::start();
    let run = continuation::Run { deadline: super::jobs::LEDGER_DEADLINE, chain_runs: continuation::CHAIN_RUNS, stopwatch: &stopwatch };
    sync_within(db, transport, key_id, credentials, clock, wait, &run).await
}

/// `sync` within `run`'s bounds.
pub async fn sync_within<T: Transport>(db: &Db, transport: T, key_id: i64, credentials: &Credentials, clock: &dyn Clock, wait: std::time::Duration, run: &continuation::Run<'_>)
                                       -> Result<Result<Outcome, Failure>, SyncError> {
    let version = capture_bound_version(db, key_id, credentials).await?;
    let now = clock.now();
    let captured = version.clone();
    let (key, produced, identity) = phase(db, move |c| {
        let key = load_key_of(c, key_id, &[KRAKEN])?;
        // A watermark another credential produced is read as none: the history is read whole (#511), and so is the first
        // Rust sync after Rails' (no provenance stamp), which is what repairs Rails' rows (D-KR-5).
        let produced = super::cache::ledger_produced_by(c, key_id, &captured)?;
        let identity = KrakenIdentity::load(c, key.exchange_id)?;
        Ok((key, produced, identity))
    }).await?;
    let watermark = if produced { key.last_synced_at } else { None };
    let venue = KrakenVenue::new(transport);
    continuation::sync(db, &version, key, watermark, chrono::Duration::hours(OVERLAP_HOURS), credentials, now, run, |begin| Walk::new(&venue, &identity, wait, begin)).await
}

// ---- the repair of rows an earlier sync wrote (D-KR-5) ---------------------------------------------------------------

/// What a stored row becomes: the normalised entry's columns and the resolver's asset.
#[derive(Clone, Debug, PartialEq)]
struct Target { entry_type: i64, base_currency: String, base_amount: BigDec, fee_currency: Option<String>, fee_amount: Option<BigDec>, base_asset_id: Option<i64> }

impl Target {
    fn json(&self, tx_id: &str) -> Value {
        json!({ "tx_id": tx_id, "entry_type": self.entry_type, "base_currency": self.base_currency, "base_amount": self.base_amount.to_s_f(),
                "fee_currency": self.fee_currency, "fee_amount": self.fee_amount.as_ref().map(BigDec::to_s_f), "base_asset_id": self.base_asset_id })
    }
    fn of(v: &Value) -> Option<(String, Self)> {
        let dec = |k: &str| v[k].as_str().and_then(|s| BigDec::parse(s).ok());
        Some((v["tx_id"].as_str()?.to_string(), Self { entry_type: v["entry_type"].as_i64()?, base_currency: v["base_currency"].as_str()?.to_string(), base_amount: dec("base_amount")?,
                                                       fee_currency: v["fee_currency"].as_str().map(str::to_string), fee_amount: dec("fee_amount"), base_asset_id: v["base_asset_id"].as_i64() }))
    }
}

/// A stored row as the repair reads it.
struct Row { id: i64, user_id: i64, exchange_id: i64, entry_type: i64, base_currency: Option<String>, base_amount: BigDec, base_asset_id: Option<i64>, tx_id: Option<String>, linked: Option<i64> }

fn row(c: &Connection, sql_where: &str, p: &[&dyn rusqlite::ToSql]) -> Result<Option<Row>, SyncError> {
    let found = c.query_row(&format!("SELECT id, entry_type, base_currency, base_amount, base_asset_id, tx_id, linked_transaction_id, user_id, exchange_id FROM account_transactions WHERE {sql_where}"), p, |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<String>>(2)?, super::number::stored(r.get_ref(3)?), r.get::<_, Option<i64>>(4)?, r.get::<_, Option<String>>(5)?, r.get::<_, Option<i64>>(6)?, r.get::<_, i64>(7)?, r.get::<_, i64>(8)?))
    }).optional()?;
    let Some((id, entry_type, base_currency, amount, base_asset_id, tx_id, linked, user_id, exchange_id)) = found else { return Ok(None) };
    Ok(Some(Row { id, user_id, exchange_id, entry_type, base_currency, base_amount: amount.map_err(SyncError)?.unwrap_or_else(BigDec::zero), base_asset_id, tx_id, linked }))
}

/// The other end of a linked row: (this row is the withdrawal, the other end's stored row).
fn counterpart(c: &Connection, r: &Row) -> Result<Option<(bool, Row)>, SyncError> {
    if let Some(dep) = r.linked { return Ok(row(c, "id = ?1", params![dep])?.map(|d| (true, d))); }
    Ok(row(c, "linked_transaction_id = ?1 LIMIT 1", params![r.id])?.map(|w| (false, w)))
}

/// AccountTransaction#linked_transaction_is_valid on the columns a repair may change: withdrawal → deposit, one currency,
/// the deposit no larger than the withdrawal.
fn valid_pair(wd: (i64, Option<&str>, &BigDec), dep: (i64, Option<&str>, &BigDec)) -> bool {
    wd.0 == WITHDRAWAL && dep.0 == DEPOSIT && wd.1.is_some() && wd.1 == dep.1 && wd.2 >= dep.2
}

fn deferred_key(key_id: i64) -> String { format!("rust_sync.kraken_repair_deferred:{key_id}") }

fn load_deferred(c: &Connection, key_id: i64) -> Result<Vec<(String, Target)>, SyncError> {
    let text: Option<String> = c.query_row("SELECT value FROM app_configs WHERE key = ?1", [deferred_key(key_id)], |r| r.get(0)).optional()?.flatten();
    let v: Value = text.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    Ok(v["rows"].as_array().into_iter().flatten().filter_map(Target::of).collect())
}

fn save_deferred(c: &Connection, key_id: i64, rows: &[(String, Target)], now: DateTime<Utc>) -> Result<(), SyncError> {
    if rows.is_empty() { c.execute("DELETE FROM app_configs WHERE key = ?1", [deferred_key(key_id)])?; return Ok(()); }
    let value = json!({ "rows": rows.iter().map(|(tx, t)| t.json(tx)).collect::<Vec<_>>() });
    c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES (?1, ?2, ?3, ?3) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
              params![deferred_key(key_id), value.to_string(), sql_time(now)])?;
    Ok(())
}

/// Writes `t` on row `id` unless it already holds it; whether it wrote.
fn write(c: &Connection, id: i64, t: &Target, now: DateTime<Utc>) -> Result<bool, SyncError> {
    let real = |d: &BigDec| d.to_f();
    let fee = t.fee_amount.as_ref().filter(|f| !f.is_zero());
    let (fee_currency, fee_amount) = (fee.and(t.fee_currency.as_deref()), fee.map(real));
    Ok(c.execute("UPDATE account_transactions SET entry_type = ?1, base_currency = ?2, base_amount = ?3, fee_currency = ?4, fee_amount = ?5, base_asset_id = ?6, updated_at = ?7 \
                  WHERE id = ?8 AND NOT (entry_type IS ?1 AND base_currency IS ?2 AND base_amount IS ?3 AND fee_currency IS ?4 AND fee_amount IS ?5 AND base_asset_id IS ?6)",
                 params![t.entry_type, t.base_currency, real(&t.base_amount), fee_currency, fee_amount, t.base_asset_id, sql_time(now), id])? == 1)
}

/// The repair of one stored row, inside `ledger::store` before its dedup (the Coinbase rekey's place), on a Kraken ledger
/// sync only (`Progress::kraken_repair`; never a file import). The row the user's earlier sync stored under this ledger id
/// is rewritten to `e` (plan §2.6):
/// 1. a stored asset that is not the resolver's wins: its currency stays, the amounts are still Kraken's;
/// 2. a missing asset is filled;
/// 3. a linked row (either end) is written only if the pair it then forms with its counterpart's stored row is valid
///    (r3 row 2): each write is valid against what is committed. Otherwise it is deferred to the import's completion (r3
///    row 5, `converge`), in this page's transaction;
/// 4. its manual values, link, rejection, description and order are never touched.
pub(super) fn repair(c: &Connection, key: &Key, e: &Entry, assets: &AssetRule, now: DateTime<Utc>) -> Result<(), SyncError> {
    let Some(tx_id) = e.tx_id.as_deref() else { return Ok(()) };
    let Some(stored) = row(c, "user_id = ?1 AND exchange_id = ?2 AND tx_id = ?3", params![key.user_id, key.exchange_id, tx_id])? else { return Ok(()) };
    let resolved = assets.asset_id(c, e.base_currency.as_deref(), &e.raw.value, e.transacted_at)?;
    let mut t = Target { entry_type: e.entry_type, base_currency: e.base_currency.clone().unwrap_or_default(), base_amount: e.base_amount.clone(),
                         fee_currency: e.fee_currency.clone(), fee_amount: e.fee_amount.clone(), base_asset_id: resolved };
    if let Some(id) = stored.base_asset_id.filter(|id| Some(*id) != resolved) {
        t.base_asset_id = Some(id);
        t.base_currency = stored.base_currency.clone().unwrap_or_default();
        if t.fee_currency.is_some() { t.fee_currency = Some(t.base_currency.clone()); }
    }
    if let Some((withdrawal, other)) = counterpart(c, &stored)? {
        let mine = (t.entry_type, Some(t.base_currency.as_str()), &t.base_amount);
        let theirs = (other.entry_type, other.base_currency.as_deref(), &other.base_amount);
        let ok = if withdrawal { valid_pair(mine, theirs) } else { valid_pair(theirs, mine) };
        if !ok {
            let mut rows = load_deferred(c, key.id)?;
            rows.retain(|(tx, _)| tx != tx_id);
            rows.push((tx_id.to_string(), t));
            return save_deferred(c, key.id, &rows, now);
        }
    }
    write(c, stored.id, &t, now)?;
    Ok(())
}

/// At the import's completion, in its finishing transaction (before the provenance stamp and the watermark): every
/// deferred row against its counterpart's joint target (its own deferred target, else its stored row). A valid pair is
/// written on both ends together; one invalid even jointly is left for good and counted. The list is then removed.
fn converge(c: &Connection, key: &Key, now: DateTime<Utc>) -> Result<(), SyncError> {
    let deferred = load_deferred(c, key.id)?;
    if deferred.is_empty() { return Ok(()); }
    let targets: HashMap<&str, &Target> = deferred.iter().map(|(tx, t)| (tx.as_str(), t)).collect();
    let (mut done, mut left) = (HashSet::new(), 0usize);
    for (tx_id, t) in &deferred {
        if done.contains(tx_id.as_str()) { continue; }
        let Some(stored) = row(c, "user_id = ?1 AND exchange_id = ?2 AND tx_id = ?3", params![key.user_id, key.exchange_id, tx_id])? else { continue };
        let Some((withdrawal, other)) = counterpart(c, &stored)? else { write(c, stored.id, t, now)?; continue };
        let joint = other.tx_id.as_deref().filter(|_| other.user_id == key.user_id && other.exchange_id == key.exchange_id)
            .and_then(|tx| targets.get(tx).map(|ot| (tx.to_string(), *ot)));
        let theirs = match &joint { Some((_, ot)) => (ot.entry_type, Some(ot.base_currency.as_str()), &ot.base_amount), None => (other.entry_type, other.base_currency.as_deref(), &other.base_amount) };
        let mine = (t.entry_type, Some(t.base_currency.as_str()), &t.base_amount);
        if if withdrawal { valid_pair(mine, theirs) } else { valid_pair(theirs, mine) } {
            write(c, stored.id, t, now)?;
            if let Some((tx, ot)) = joint { write(c, other.id, ot, now)?; done.insert(tx); }
        } else {
            left += 1;
        }
        done.insert(tx_id.clone());
    }
    if left > 0 { crate::engine::log(&format!("[kraken] ledger import of api key {}: kraken repair: {left} linked rows left", key.id)); }
    save_deferred(c, key.id, &[], now)
}

// ---- internal pairs (D-KR-4) -----------------------------------------------------------------------------------------

/// The user's internal Kraken pairs to link: per (venue, refid), exactly one unlinked, unrejected internal withdrawal and
/// one unclaimed internal deposit of one currency, the deposit no larger and no earlier. As (deposit, withdrawal).
pub(super) fn internal_pairs(c: &Connection, user_id: i64) -> Result<Vec<(i64, i64)>, SyncError> {
    let mut s = c.prepare(&format!(
        "SELECT id, exchange_id, group_id, entry_type, base_currency, base_amount, transacted_at, linked_transaction_id IS NOT NULL OR transfer_link_rejected = 1 \
         OR EXISTS (SELECT 1 FROM account_transactions l WHERE l.linked_transaction_id = account_transactions.id) \
         FROM account_transactions WHERE user_id = ?1 AND entry_type IN ({DEPOSIT}, {WITHDRAWAL}) AND group_id IS NOT NULL AND {} ORDER BY id", internal_leg_sql("")))?;
    type Leg = (i64, i64, String, i64, Option<String>, BigDec, String, bool);
    let mut legs: Vec<Leg> = vec![];
    let mut q = s.query([user_id])?;
    while let Some(r) = q.next()? {
        let amount = super::number::stored(r.get_ref(5)?).map_err(SyncError)?.unwrap_or_else(BigDec::zero);
        legs.push((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, amount, r.get(6)?, r.get(7)?));
    }
    let mut groups: HashMap<(i64, &str), Vec<&Leg>> = HashMap::new();
    for leg in &legs { groups.entry((leg.1, leg.2.as_str())).or_default().push(leg); }
    let mut pairs = vec![];
    for group in groups.values() {
        let (wds, deps): (Vec<&&Leg>, Vec<&&Leg>) = group.iter().partition(|l| l.3 == WITHDRAWAL);
        let ([wd], [dep]) = (&wds[..], &deps[..]) else { continue };
        if wd.7 || dep.7 || wd.4.is_none() || wd.4 != dep.4 || wd.5 < dep.5 || dep.6 < wd.6 { continue; }
        pairs.push((dep.0, wd.0));
    }
    pairs.sort_unstable();
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> KrakenIdentity {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE assets(id INTEGER PRIMARY KEY, external_id TEXT, symbol TEXT, instrument_type TEXT);
            CREATE TABLE tickers(id INTEGER PRIMARY KEY, exchange_id INTEGER, base TEXT, quote TEXT, base_asset_id INTEGER, quote_asset_id INTEGER);
            INSERT INTO assets VALUES (1,'usd','USD',NULL),(2,'bitcoin','BTC',NULL),(3,'litecoin','LTC',NULL),(4,'polkadot','DOT',NULL),
              (5,'nvidia-xstock','NVDAx','tokenized'),(6,'solana','SOL',NULL),(7,'matic-network','MATIC',NULL),(8,'twin-a','TWIN',NULL),(9,'twin-b','TWIN',NULL),
              (10,'terra-luna-2','LUNA',NULL),(11,'brk-xstock','BRK.Bx','tokenized');
            INSERT INTO tickers VALUES (1,1,'XBT','USD',2,1),(2,1,'LTC','USD',3,1),(3,1,'DOT','USD',4,1),(4,1,'NVDAx','USD',5,1),(5,1,'SOL','USD',6,1),
              (6,1,'POL','USD',7,1),(7,1,'TWINA','USD',8,1),(8,1,'TWINB','USD',9,1),(9,1,'__stale_3_LUNA','USD',10,1),(10,1,'BRK.Bx','USD',11,1);").unwrap();
        KrakenIdentity::load(&c, 1).unwrap()
    }

    #[test]
    fn every_step_of_the_identity() {
        let k = identity();
        for (code, want) in [("XBT", Some(2)), ("XXBT", Some(2)), ("ZUSD", Some(1)), ("USD", Some(1)), ("XLTC", Some(3)), ("DOT.S", Some(4)), ("DOT.F", Some(4)),
                             ("DOT.P", Some(4)), ("XLTC.S", Some(3)), ("DOT.S.S", None), ("NVDAx", Some(5)), ("NVDASPV", Some(5)), ("NVDAx.T", Some(5)),
                             ("SOL.T", None), ("BRK.Bx", Some(11)), ("MATIC", Some(7)), ("LUNA", Some(10)), ("BTC", Some(2)), ("TWIN", None), ("ZZZ", None)] {
            assert_eq!(k.asset_id(code), want, "{code}");
        }
        assert_eq!(k.identify("XXBT", None), ("BTC".into(), Some(2)));
        assert_eq!(k.identify("DOT.S", None), ("DOT".into(), Some(4)));
        assert_eq!(k.identify("NVDASPV", None), ("NVDAx".into(), Some(5)));
        assert_eq!(k.identify("XXDG", None), ("XDG".into(), None));
        assert_eq!(k.identify("XBT.M", None), ("BTC".into(), Some(2)));
        assert_eq!(k.identify("ZZZ.S", None), ("ZZZ.S".into(), None));
    }

    fn entry(kind: &str, subtype: &str, amount: &str, fee: &str) -> (i64, String, Option<String>, Option<String>) {
        let raw = Raw::from_value(json!({ "refid": "R", "time": 1758331800.5, "type": kind, "subtype": subtype, "aclass": "currency", "asset": "XXBT", "amount": amount, "fee": fee, "balance": "1" }));
        let e = normalise("L1", raw, &identity()).unwrap();
        (e.entry_type, e.base_amount.to_s_f(), e.fee_amount.map(|f| f.to_s_f()), e.description)
    }

    /// Plan §2.3 with r3 row 1: every case of both tables, positive, negative and zero.
    #[test]
    fn the_normaliser_is_total_over_the_sign() {
        let t = |k: &str, a: &str, f: &str| { let (ty, b, fee, _) = entry(k, "", a, f); (ty, b, fee) };
        let s = |b: &str| b.to_string();
        // Trade group: a positive fee is kept; a negative one is folded.
        assert_eq!(t("trade", "1", "0.01"), (BUY, s("1.0"), Some(s("0.01"))));
        assert_eq!(t("spend", "-1", "0.01"), (SELL, s("1.0"), Some(s("0.01"))));
        assert_eq!(t("receive", "0.01", "0.02"), (SELL, s("0.01"), None));
        assert_eq!(t("adjustment", "0", "0.02"), (SELL, s("0.02"), None));
        assert_eq!(t("conversion", "0", "0"), (UNSUPPORTED_ACTIVITY, s("0.0"), None));
        assert_eq!(t("trade", "1", "-0.01"), (BUY, s("1.01"), None));
        assert_eq!(t("trade", "-1", "-0.01"), (SELL, s("0.99"), None));
        assert_eq!(t("trade", "0", "-0.01"), (BUY, s("0.01"), None));
        assert_eq!(t("trade", "-0.01", "-0.01"), (UNSUPPORTED_ACTIVITY, s("0.0"), None));
        // Every other type: the fee folded, the sign decides.
        for (kind, sub, up, down) in [("deposit", "", DEPOSIT, WITHDRAWAL), ("withdrawal", "", DEPOSIT, WITHDRAWAL), ("staking", "", STAKING, WITHDRAWAL),
                                      ("reward", "", STAKING, WITHDRAWAL), ("earn", "reward", STAKING, WITHDRAWAL), ("dividend", "", OTHER_INCOME, WITHDRAWAL),
                                      ("rollover", "", DEPOSIT, FEE), ("creator_fee", "", DEPOSIT, FEE), ("nftcreatorfee", "", DEPOSIT, FEE),
                                      ("earn", "allocation", DEPOSIT, WITHDRAWAL), ("earn", "deallocation", DEPOSIT, WITHDRAWAL), ("earn", "autoallocate", DEPOSIT, WITHDRAWAL),
                                      ("earn", "migration", DEPOSIT, WITHDRAWAL), ("transfer", "spottostaking", DEPOSIT, WITHDRAWAL), ("transfer", "stakingfromspot", DEPOSIT, WITHDRAWAL),
                                      ("transfer", "stakingtospot", DEPOSIT, WITHDRAWAL), ("transfer", "spotfromstaking", DEPOSIT, WITHDRAWAL),
                                      ("transfer", "spottofutures", DEPOSIT, WITHDRAWAL), ("transfer", "spotfromfutures", DEPOSIT, WITHDRAWAL),
                                      ("transfer", "airdrop", AIRDROP, WITHDRAWAL), ("transfer", "", AIRDROP, WITHDRAWAL), ("margin", "", DEPOSIT, WITHDRAWAL),
                                      ("settled", "", DEPOSIT, WITHDRAWAL), ("credit", "", DEPOSIT, WITHDRAWAL), ("sale", "", DEPOSIT, WITHDRAWAL),
                                      ("nft_trade", "", DEPOSIT, WITHDRAWAL), ("custodytransfer", "", DEPOSIT, WITHDRAWAL), ("none", "", DEPOSIT, WITHDRAWAL),
                                      ("mystery", "x", DEPOSIT, WITHDRAWAL)] {
            let pos = entry(kind, sub, "2", "0.5");
            assert_eq!((pos.0, pos.1.as_str(), pos.2.clone()), (up, "1.5", None), "{kind}/{sub} +");
            let neg = entry(kind, sub, "-2", "0.5");
            assert_eq!((neg.0, neg.1.as_str(), neg.2.clone()), (down, "2.5", None), "{kind}/{sub} -");
            let zero = entry(kind, sub, "0.5", "0.5");
            assert_eq!((zero.0, zero.1.as_str()), (UNSUPPORTED_ACTIVITY, "0.0"), "{kind}/{sub} 0");
            let label = if sub.is_empty() { format!("Kraken {kind}") } else { format!("Kraken {kind}/{sub}") };
            assert_eq!(pos.3, Some(label), "{kind}/{sub}");
        }
        assert_eq!(entry("trade", "", "1", "0").3, None, "a trade leg has no description");
    }

    #[test]
    fn a_page_reads_its_entries_and_cursor() {
        let (rows, next) = read_page(r#"{"error":[],"result":{"ledger":{"L1":{"asset":"XXBT"},"L2":{"asset":"ZUSD"}},"count":null,"cursor":{"next":"c2"}}}"#).ok().unwrap();
        assert_eq!((rows.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), next.as_deref()), (vec!["L1", "L2"], Some("c2")));
        for last in [r#"{"error":[],"result":{"ledger":{}}}"#, r#"{"error":[],"result":{"ledger":[],"cursor":{"next":null}}}"#, r#"{"error":[]}"#] {
            let (rows, next) = read_page(last).ok().unwrap();
            assert!(rows.is_empty() && next.is_none(), "{last}");
        }
        let Err(f) = read_page(r#"{"error":["EAPI:Rate limit exceeded"]}"#) else { panic!() };
        assert_eq!((f.fail, f.venue), (("EAPI:Rate limit exceeded".to_string(), false), true));
        let Err(f) = read_page("maintenance") else { panic!() };
        assert_eq!((f.fail, f.venue), (("maintenance".to_string(), false), false));
    }
}
