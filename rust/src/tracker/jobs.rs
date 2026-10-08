//! The tracker's jobs under the scheduler (`crate::jobs`), per user with a reading Alpaca key:
//! - `tracker_ledger` (Tracker::LedgerJob): the walk, today's snapshot rows (PortfolioSnapshot.record!) and the
//!   wash-sale locks. On demand only: the ledger and balance syncs wake it after every run, as Rails' sync jobs end
//!   in a LedgerJob or a record!, and so does the backfill.
//! - `portfolio_backfill` (PortfolioSnapshot::BackfillJob): every earlier day. Rails asks for it from the tracker
//!   page, which this build does not serve yet; here it runs at 03:00 UTC, after the nightly syncs, when the page's
//!   own test says a sweep is wanted.
//!
//! The walk reads the database only. A price Rails would fetch first stops it: the run fetches the range from
//! data-api, stores it, and walks again (`with_walk`). One job run, every pass and every load of it, spends from one
//! `Allowance`: `WALK`'s steps and `MAX_FETCHES` fetches. A run that spends it stops with the reason; the prices it
//! stored stay, and the next run goes on from them.
use super::backfill::{self, Plan};
use super::prices::{self, Fetch, Halt, PriceBook, Venue};
use super::walk::{self, Walked};
use crate::crypto::Cipher;
use crate::engine::Clock;
use crate::figures::{budget, FiguresError};
use crate::jobs::data_api::DataApi;
use crate::jobs::schedule::{Jitter, Schedule};
use crate::jobs::{Cx, Db, Job, JobFuture, Outcome, Retry, Spec, Wake, DEADLINE};
use crate::sync::jobs::Connect;
use crate::venue::http::Transport;
use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::Connection;
use std::collections::HashMap;
use std::rc::Rc;

pub const TRACKER_LEDGER: &str = "tracker_ledger";
pub const PORTFOLIO_BACKFILL: &str = "portfolio_backfill";

/// What one job run may cost, all its passes together: the figures' own limits (`budget::FIGURE`). Measured on a
/// debug build: 5,000 rows of buys, sales and dividends take 2.3 million steps and hold 10,665 limbs
/// (`walk::tests::a_long_history_…`). Steps add up over the run; held limbs are a peak, freed between passes.
pub const WALK: budget::Limits = budget::FIGURE;
/// How many single-day fetches one job run may halt for. ponytail: a history missing more days than this fills in
/// over several runs (each stores what it fetched); raise it if real histories need more.
pub const MAX_FETCHES: usize = 50;

/// What is left of one job run's budget.
#[derive(Clone, Copy, Debug)]
pub struct Allowance { pub steps: u64, pub fetches: usize }

impl Allowance {
    pub fn run() -> Allowance { Allowance { steps: WALK.steps, fetches: MAX_FETCHES } }
}

/// Why a run stopped at its fetch allowance.
pub const FETCHES_SPENT: &str = "the tracker walk needs more prices than one run fetches; the next run fetches the rest";

pub fn message(e: FiguresError) -> String {
    match e {
        FiguresError::Sqlite(e) => format!("database: {e}"),
        FiguresError::Data(m) | FiguresError::NotComputed(m) | FiguresError::Raised(m) => m,
    }
}

/// Fetches one range unless the table covers it, and stores what came back (one write unit). With no data-api
/// configured, the install's provider is CoinGecko, which this build does not fetch from: a range that needs a request
/// refuses the walk.
async fn fetch<T: Transport>(db: &Db, api: Option<&DataApi<T>>, f: Fetch) -> Result<(), String> {
    let (asset, from, to) = (f.symbol.clone(), f.from, f.to);
    let have = db.run(move |c, _| prices::stored_days(c, &asset, from, to).map_err(message)).await?;
    let Some(api) = api else {
        return if prices::covered(&f, &have) { Ok(()) } else { Err(message(super::refused("historical prices from CoinGecko"))) };
    };
    let rows = prices::fetch_range(api, &f, &have).await?;
    if rows.is_empty() { return Ok(()); }
    db.run(move |c, _| {
        c.execute_batch("BEGIN IMMEDIATE").map_err(|e| e.to_string())?;
        prices::store(c, &rows).map_err(message)?;
        c.execute_batch("COMMIT").map_err(|e| e.to_string())
    }).await
}

/// `work` on the blocking pool, in a budget scope of what the run has left; what it took is spent.
pub async fn metered<R: Send + 'static>(db: &Db, allowance: &mut Allowance,
    work: impl FnOnce(&Connection, &Cipher) -> Result<R, FiguresError> + Send + 'static) -> Result<R, String> {
    let limits = budget::Limits { steps: allowance.steps, held: WALK.held };
    let (out, used) = db.run(move |c, cipher| Ok(budget::scope(limits, || work(c, cipher)))).await?;
    allowance.steps = allowance.steps.saturating_sub(used.steps);
    out.map_err(message)
}

/// The wall clock a write is dated by, read once the write lock is held (`written`). The jobs read the system's; a
/// test or the parity harness reads its own.
pub type Wall = std::sync::Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

pub fn system_wall() -> Wall { std::sync::Arc::new(Utc::now) }

/// What a pass hands its result to: the connection, the rows walked, the walk, and the job clock's reading as the
/// pass began.
type Then<'a, R> = &'a dyn Fn(&Connection, &Cipher, &[super::rows::Stored], &Walked, DateTime<Utc>) -> Result<R, FiguresError>;

/// One pass of a walk: the rows loaded, priced and walked, then `then` on the result. A price that must be fetched
/// first ends the pass with that fetch.
fn pass<R>(c: &Connection, cipher: &Cipher, user_id: i64, fetched: &HashMap<(String, NaiveDate), u32>, now: DateTime<Utc>,
           then: Then<R>) -> Result<Result<R, Fetch>, FiguresError> {
    let today = now.date_naive();
    let rows = super::rows::load(c, user_id)?;
    let venue = Venue::alpaca(c)?;
    let reference = prices::Reference::load(c, &venue, &prices::symbols(&rows))?;
    let mut book = PriceBook { r: &reference, venue, today, fetched, warnings: 0 };
    let prepared = match walk::prepare(&rows, &mut book) {
        Ok(p) => p,
        Err(Halt::Fetch(f)) => return Ok(Err(f)),
        Err(Halt::Fail(e)) => return Err(e),
    };
    let walked = walk::walk(&prepared, book.warnings, today)?;
    Ok(Ok(then(c, cipher, &rows, &walked, now)?))
}

/// One write unit (`BEGIN IMMEDIATE` … `COMMIT`, rolled back on an error), dated by the wall clock read once the
/// write lock is held, after every load and walk before it, as Rails' `Date.current` is read when the row is built.
pub fn written<R>(c: &Connection, wall: &dyn Fn() -> DateTime<Utc>, write: impl FnOnce(&Connection, DateTime<Utc>) -> Result<R, FiguresError>) -> Result<R, FiguresError> {
    c.execute_batch("BEGIN IMMEDIATE")?;
    match write(c, wall()) {
        Ok(out) => { c.execute_batch("COMMIT")?; Ok(out) }
        Err(e) => { let _ = c.execute_batch("ROLLBACK"); Err(e) }
    }
}

/// The walk of one user's ledger as Rails' `walk(user)` makes it, then `then` on its result: the prices Rails
/// prefetches fetched first, then each price a pass halts for, while the run's allowance lasts.
pub async fn with_walk<T: Transport, R: Send + 'static>(db: &Db, api: Option<&DataApi<T>>, user_id: i64, clock: &dyn Clock, allowance: &mut Allowance,
    then: impl Fn(&Connection, &Cipher, &[super::rows::Stored], &Walked, DateTime<Utc>) -> Result<R, FiguresError> + Send + Sync + 'static) -> Result<R, String> {
    let wanted = metered(db, allowance, move |c, _| {
        let rows = super::rows::load(c, user_id)?;
        let venue = Venue::alpaca(c)?;
        prices::prefetch(&prices::Reference::load(c, &venue, &prices::symbols(&rows))?, &rows, &venue)
    }).await?;
    for f in wanted { fetch(db, api, f).await?; }
    let then = std::sync::Arc::new(then);
    let mut fetched: HashMap<(String, NaiveDate), u32> = HashMap::new();
    loop {
        let (seen, then, now) = (fetched.clone(), then.clone(), clock.now());
        match metered(db, allowance, move |c, cipher| pass(c, cipher, user_id, &seen, now, &*then)).await? {
            Ok(done) => return Ok(done),
            Err(f) => {
                if allowance.fetches == 0 { return Err(FETCHES_SPENT.into()); }
                allowance.fetches -= 1;
                if let Some(day) = f.asked { *fetched.entry((f.symbol.clone(), day)).or_insert(0) += 1; }
                fetch(db, api, f).await?;
            }
        }
    }
}

/// Tracker::LedgerJob#perform: the walk, today's snapshot rows from it, and the wash-sale locks, the two writes in one
/// unit, dated when they are written. Returns the walk. The run spends from `allowance` (`Allowance::run()` for a job).
pub async fn ledger_run<T: Transport>(db: &Db, api: Option<&DataApi<T>>, user_id: i64, clock: &dyn Clock, wall: Wall, allowance: &mut Allowance) -> Result<Walked, String> {
    with_walk(db, api, user_id, clock, allowance, move |c, _, _, walked, began| {
        // Everything read and computed first, so the write lock is held for the writes alone.
        let rows = super::snapshot::today_rows(c, user_id, walked)?;
        let locks = super::wash::targets(c, user_id, &walked.whole.loss_sales)?;
        written(c, &*wall, |c, now| {
            super::snapshot::write(c, user_id, &rows, now.date_naive())?;
            // The lock rows' timestamps are the pass's clock reading, as Rails' frozen `Time.current` is in one job.
            super::wash::confirm_all(c, user_id, &locks, began)?;
            Ok(walked.clone())
        })
    }).await
}

/// PortfolioSnapshot::BackfillJob#perform. Returns whether a history was written (none before the first transaction's
/// day has ended).
pub async fn backfill_run<C: Connect, T: Transport>(db: &Db, venues: &C, api: Option<&DataApi<T>>, user_id: i64, clock: &dyn Clock) -> Result<bool, String> {
    let now = clock.now();
    let today = now.date_naive();
    let last = today.pred_opt().unwrap_or(today);
    let mut allowance = Allowance::run();
    // Read before the rows are, so a row landing mid-sweep leaves the history stale.
    let begun = metered(db, &mut allowance, move |c, cipher| {
        let version = backfill::history_version(c, user_id)?;
        let rows = super::rows::load(c, user_id)?;
        let first = rows.iter().map(|r| r.date()).min();
        match first {
            Some(first) if first <= last => Ok(Some((version, first, backfill::plan(c, user_id, &rows, first, last)?))),
            _ => { crate::app_config::set(c, cipher, &backfill::history_key(user_id), &version, now).map_err(FiguresError::Data)?; Ok(None) }
        }
    }).await?;
    let Some((version, first, plan)) = begun else { return Ok(false) };
    fetch_closes(db, venues, api, &plan).await?;
    let (plan, version) = (std::sync::Arc::new(plan), std::sync::Arc::new(version));
    with_walk(db, api, user_id, clock, &mut allowance, move |c, cipher, rows, walked, _| {
        let swept = backfill::sweep(c, &plan, rows, &walked.terms, first, last)?;
        backfill::store(c, cipher, user_id, &swept, last, &version, now)?;
        Ok(true)
    }).await
}

/// `fetch_missing` for every instrument: each coin's ranges from data-api, each stock's closes from the broker.
async fn fetch_closes<C: Connect, T: Transport>(db: &Db, venues: &C, api: Option<&DataApi<T>>, plan: &Plan) -> Result<(), String> {
    // Every bar request's key and stored closes, read once before the requests.
    let bars: Vec<backfill::Bars> = plan.fetches.iter().filter_map(|f| match f { backfill::Close::Bars(b) => Some(b.clone()), _ => None }).collect();
    let wanted = bars.clone();
    let (keys, stored) = db.run(move |c, cipher| {
        let mut keys = HashMap::new();
        for b in &wanted {
            if let std::collections::hash_map::Entry::Vacant(e) = keys.entry(b.api_key_id) { e.insert(crate::sync::credentials(c, cipher, b.api_key_id).map_err(|e| e.0)?); }
        }
        let venue = Venue::alpaca(c).map_err(message)?;
        let reference = prices::Reference::load(c, &venue, &wanted.iter().map(|b| b.symbol.clone()).collect()).map_err(message)?;
        let stored: HashMap<String, std::collections::HashSet<NaiveDate>> = wanted.iter()
            .map(|b| (b.symbol.clone(), reference.prices_over(&format!("stock:{}", b.symbol), b.from, b.to).map(|(d, _)| *d).collect())).collect();
        Ok((keys, stored))
    }).await?;
    for close in &plan.fetches {
        let bars = match close { backfill::Close::Range(f) => { fetch(db, api, f.clone()).await?; continue } backfill::Close::Bars(b) => b };
        let (Some(credentials), Some(have)) = (keys.get(&bars.api_key_id), stored.get(&bars.symbol)) else { continue };
        let start = format!("{}T00:00:00Z", bars.from);
        // `stock_price_range`: any failure of the request leaves the symbol without closes.
        let Ok(body) = venues.connect(credentials).read(true, &format!("/v2/stocks/{}/bars", bars.symbol),
                                                        vec![("limit", "10000".into()), ("start", start), ("timeframe", "1Day".into())], 8 * 1024 * 1024).await else { continue };
        let Ok(text) = body.text() else { continue };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(text) else { continue };
        let Some(rows) = backfill::bar_rows(&json, bars, have) else { continue };
        if rows.is_empty() { continue; }
        db.run(move |c, _| {
            c.execute_batch("BEGIN IMMEDIATE").map_err(|e| e.to_string())?;
            prices::store(c, &rows).map_err(message)?;
            c.execute_batch("COMMIT").map_err(|e| e.to_string())
        }).await?;
    }
    Ok(())
}

/// `tracker_ledger`, scoped by the user.
pub struct LedgerWalk<T: Transport> { api: Rc<Option<DataApi<T>>>, user_id: i64, wall: Wall }

impl<T: Transport> Job for LedgerWalk<T> {
    fn spec(&self) -> Spec {
        Spec { name: TRACKER_LEDGER, scope: Some(self.user_id.to_string()), schedule: None, jitter: Jitter::NONE, retry: Retry::None, deadline: DEADLINE }
    }
    fn run<'a>(&'a self, cx: Cx<'a>, _wakes: Vec<Wake>) -> JobFuture<'a> {
        Box::pin(async move {
            match cached_ledger_run(&cx, (*self.api).as_ref(), self.user_id, self.wall.clone()).await {
                Ok(_) => { cx.db.notifications.ledger_done(self.user_id); Outcome::Done }
                Err(e) => Outcome::Failed(e),
            }
        })
    }
}

/// Publish only a walk whose transaction/price version still matches inside the cache write.
/// The three passes share the existing walk budget. A changing account is retried by its
/// scheduler wake; no GET starts a calculation. Snapshot and lock behavior stays in ledger_run.
const CACHE_MOVING: &str = "Tracker ledger unavailable: inputs changed during calculation";
async fn cached_ledger_run<T:Transport>(cx:&Cx<'_>,api:Option<&DataApi<T>>,owner:i64,wall:Wall)->Result<Walked,String> {
    let mut allowance=Allowance::run();
    for _ in 0..3 {
        let (before,venue)=cx.db.run(move|c,_|Ok((super::cache::version(c,owner).map_err(message)?,Venue::alpaca(c).map_err(message)?.id))).await?;
        let result=ledger_run(&cx.db,api,owner,cx.clock,wall.clone(),&mut allowance).await;
        let cached=match &result {Ok(walked)=>Some(walked.clone()),Err(_)=>None};
        let clock=wall.clone();
        let published=cx.db.run(move|c,_|written(c,&*clock,|c,now|super::cache::publish(c,owner,&before,cached.as_ref(),venue,now)).map_err(message)).await?;
        if published {return result;}
    }
    cx.wakers.wake(TRACKER_LEDGER,Some(&owner.to_string()),None);
    Err(CACHE_MOVING.into())
}

/// `portfolio_backfill`, scoped by the user.
pub struct Backfill<C: Connect, T: Transport> { venues: C, api: Rc<Option<DataApi<T>>>, user_id: i64 }

impl<C: Connect, T: Transport> Job for Backfill<C, T> {
    fn spec(&self) -> Spec {
        Spec { name: PORTFOLIO_BACKFILL, scope: Some(self.user_id.to_string()), schedule: Some(Schedule::Daily { hour: 3, minute: 0 }),
               jitter: Jitter::NONE, retry: Retry::None, deadline: DEADLINE }
    }
    fn run<'a>(&'a self, cx: Cx<'a>, _wakes: Vec<Wake>) -> JobFuture<'a> {
        Box::pin(async move {
            let (user_id, today) = (self.user_id, cx.clock.now().date_naive());
            match cx.db.run(move |c, cipher| backfill::wanted(c, cipher, user_id, today).map_err(message)).await {
                Ok(false) => return Outcome::NothingNew,
                Ok(true) => {}
                Err(e) => return Outcome::Failed(e),
            }
            match backfill_run(&cx.db, &self.venues, (*self.api).as_ref(), self.user_id, cx.clock).await {
                // The prices that just arrived are the ones the ledger was missing: it walks again.
                Ok(true) => { cx.wakers.wake(TRACKER_LEDGER, Some(&self.user_id.to_string()), None); Outcome::Done }
                Ok(false) => Outcome::Done,
                Err(e) => Outcome::Failed(e),
            }
        })
    }
}

/// Construct a consumer for a user discovered after startup (including historical-only users).
pub fn resolve<C: Connect + Clone + 'static,T: Transport + 'static>(name:&str,user_id:i64,venues:&C,api:Rc<Option<DataApi<T>>>,wall:Wall)->Option<Box<dyn Job>>{
    match name{
        TRACKER_LEDGER=>Some(Box::new(LedgerWalk{api,user_id,wall})),
        PORTFOLIO_BACKFILL=>Some(Box::new(Backfill{venues:venues.clone(),api,user_id})),
        _=>None,
    }
}

/// The tracker's jobs, per user with a reading Alpaca key, every walk before every backfill; today's rows dated by
/// `wall` (`system_wall()` in the binary).
pub fn register<C: Connect + Clone + 'static, T: Transport + 'static>(c: &Connection, venues: &C, api: Rc<Option<DataApi<T>>>, wall: Wall) -> Result<Vec<Box<dyn Job>>, String> {
    let (mut users, mut seen): (Vec<i64>, std::collections::HashSet<i64>) = (vec![], std::collections::HashSet::new());
    let mut s = c.prepare("SELECT id, user_id FROM api_keys").map_err(|e| e.to_string())?;
    let owners: HashMap<i64, i64> = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    for key in crate::sync::reading_keys(c).map_err(|e| e.0)? {
        let Some(&user) = owners.get(&key) else { continue };
        if seen.insert(user) { users.push(user); }
    }
    let mut jobs: Vec<Box<dyn Job>> = vec![];
    for user_id in &users { jobs.push(Box::new(LedgerWalk { api: api.clone(), user_id: *user_id, wall: wall.clone() })); }
    for user_id in &users { jobs.push(Box::new(Backfill { venues: venues.clone(), api: api.clone(), user_id: *user_id })); }
    Ok(jobs)
}

/// What a sync job calls when its run ends: the tracker of the key's user walks again.
pub async fn wake_for_key(cx: &Cx<'_>, key_id: i64) {
    if let Ok(user) = cx.db.run(move |c, _| c.query_row("SELECT user_id FROM api_keys WHERE id = ?1", [key_id], |r| r.get::<_, i64>(0)).map_err(|e| e.to_string())).await {
        cx.wakers.wake(TRACKER_LEDGER, Some(&user.to_string()), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::EncryptionKeys;
    use crate::engine::FixedClock;
    use crate::jobs::data_api::Config;
    use crate::venue::http::ScriptedTransport;
    use serde_json::{json, Value};

    fn day(s: &str) -> NaiveDate { NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap() }
    fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
    /// data-api's answer pricing `days`.
    fn prices(days: &[(NaiveDate, f64)]) -> Value {
        json!({ "status": 200, "body": { "prices": days.iter().map(|(d, p)| json!([d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis(), p])).collect::<Vec<_>>() } })
    }
    fn data_api(answers: Vec<Value>) -> (DataApi<ScriptedTransport>, ScriptedTransport) {
        let t = ScriptedTransport::from_script(&json!({ "GET /api/v1/historical_prices": answers }));
        (DataApi::new(Config { url: "http://data-api:3000".into(), token: "t".into() }, t.clone(), t.clone()), t)
    }

    /// An Alpaca install listing AAPL and BTC, with `rows`: (entry type, symbol, amount, its price in USD, time).
    fn install(rows: &[(i64, &str, &str, Option<&str>, &str)]) -> Db { install_on(Connection::open_in_memory().unwrap(), rows) }

    fn install_on(c: Connection, rows: &[(i64, &str, &str, Option<&str>, &str)]) -> Db {
        c.execute_batch("CREATE TABLE exchanges (id INTEGER PRIMARY KEY, type TEXT); INSERT INTO exchanges VALUES (1, 'Exchanges::Alpaca');
            CREATE TABLE assets (id INTEGER PRIMARY KEY, external_id TEXT, category TEXT, symbol TEXT, market_cap_rank INTEGER);
            INSERT INTO assets VALUES (1, 'AAPL.US', 'Stock', 'AAPL', NULL), (2, 'bitcoin', 'Cryptocurrency', 'BTC', 1);
            CREATE TABLE tickers (id INTEGER PRIMARY KEY, exchange_id INTEGER, base TEXT, base_asset_id INTEGER); INSERT INTO tickers VALUES (1, 1, 'AAPL', 1), (2, 1, 'BTC', 2);
            CREATE TABLE account_balances (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER);
            CREATE TABLE historical_prices (id INTEGER PRIMARY KEY, asset TEXT, currency TEXT, date TEXT, price NUMERIC, UNIQUE (asset, currency, date));
            CREATE TABLE account_transactions (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER, entry_type INTEGER, base_currency TEXT, base_amount NUMERIC,
                quote_currency TEXT, quote_amount NUMERIC, fee_currency TEXT, fee_amount NUMERIC, tx_id TEXT, group_id TEXT, transacted_at TEXT, raw_data TEXT,
                manual_values TEXT, linked_transaction_id INTEGER);").unwrap();
        for (kind, base, amount, usd, when) in rows {
            c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, transacted_at, raw_data, manual_values) \
                       VALUES (1, 1, ?1, ?2, ?3, CASE WHEN ?4 IS NULL THEN NULL ELSE 'USD' END, ?4, ?5, '{}', '{}')", rusqlite::params![kind, base, amount, usd, when]).unwrap();
        }
        Db::new(c, Cipher::new(&EncryptionKeys::resolve(&|_| None, "tracker-jobs").unwrap()))
    }

    /// A sale before any purchase: the walk opens AAPL a second before it, at 9 September's price.
    const SOLD_FIRST: [(i64, &str, &str, Option<&str>, &str); 2] = [(1, "AAPL", "1", Some("200"), "2026-09-10 00:00:00"), (0, "AAPL", "3", Some("585"), "2026-09-11 14:30:00")];

    /// The review's trigger: the opening's first fetch comes back empty and the second prices it above the sale, so the
    /// sale is a loss and arms its lock, as Rails' second lookup does. Two empty fetches refuse the walk, never a zero
    /// basis.
    #[tokio::test(flavor = "current_thread")]
    async fn a_failed_price_is_fetched_again_and_never_walked_at_zero() {
        let (api, t) = data_api(vec![prices(&[]), prices(&[(day("2026-09-09"), 250.0)])]);
        let db = install(&SOLD_FIRST);
        let losses = with_walk(&db, Some(&api), 1, &FixedClock(at("2026-10-01T03:00:00Z")), &mut Allowance::run(), |_, _, _, w, _| Ok(w.whole.loss_sales.clone())).await;
        assert_eq!(losses.unwrap(), [("AAPL".to_string(), day("2026-09-10"))]);
        assert_eq!(t.requests().len(), 2);
        let (api, t) = data_api(vec![prices(&[])]);
        let out = with_walk(&install(&SOLD_FIRST), Some(&api), 1, &FixedClock(at("2026-10-01T03:00:00Z")), &mut Allowance::run(), |_, _, _, _, _| Ok(())).await;
        assert!(out.as_ref().is_err_and(|e| e.starts_with("no price of AAPL on 2026-09-09 after 2 fetches")), "{out:?}");
        assert_eq!(t.requests().len(), 2);
    }

    const BOUGHT: [(i64, &str, &str, Option<&str>, &str); 2] = [(4, "USD", "1000", None, "2026-09-01 14:00:00"), (0, "AAPL", "2", Some("400"), "2026-09-02 14:30:00")];

    /// The day a row is recorded on is the wall clock's once the write lock is held: a pass that begins at 23:59:59.99
    /// and whose loading and walking cross midnight records the new day, and so does a write that waits past
    /// midnight for the lock.
    #[tokio::test(flavor = "current_thread")]
    async fn a_walk_that_crosses_midnight_records_the_day_of_the_write() {
        let late = at("2026-09-30T23:59:59.990Z");
        let wall = move |from: std::time::Instant| -> Wall { std::sync::Arc::new(move || late + chrono::Duration::from_std(from.elapsed()).unwrap()) };
        // 20,000 purchases: the pass loads, prices and walks them before it writes.
        let db = install(&BOUGHT);
        db.run(|c, _| c.execute_batch("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 20000)
            INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, transacted_at, raw_data, manual_values)
            SELECT 1, 1, 0, 'AAPL', 0.001, 'USD', 0.2, '2026-09-03 14:30:00', '{}', '{}' FROM n").map_err(|e| e.to_string())).await.unwrap();
        let clock = wall(std::time::Instant::now());
        let dated = with_walk(&db, None::<&DataApi<ScriptedTransport>>, 1, &FixedClock(late), &mut Allowance::run(),
                              move |c, _, _, _, began| written(c, &*clock, |_, now| Ok((began.date_naive(), now.date_naive())))).await;
        assert_eq!(dated, Ok((day("2026-09-30"), day("2026-10-01"))), "the pass began before midnight; its work crossed it");
        // Another writer holds the lock for 50 ms past the pass.
        let file = tempfile::NamedTempFile::new().unwrap();
        let c = Connection::open(file.path()).unwrap();
        c.execute_batch("PRAGMA busy_timeout = 5000").unwrap();
        let db = install_on(c, &BOUGHT);
        let (held, release) = (std::sync::mpsc::channel(), file.path().to_path_buf());
        let writer = std::thread::spawn(move || {
            let other = Connection::open(release).unwrap();
            other.execute_batch("BEGIN IMMEDIATE").unwrap();
            held.0.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));
            other.execute_batch("COMMIT").unwrap();
        });
        held.1.recv().unwrap();
        let clock = wall(std::time::Instant::now());
        let dated = with_walk(&db, None::<&DataApi<ScriptedTransport>>, 1, &FixedClock(late), &mut Allowance::run(),
                              move |c, _, _, _, _| written(c, &*clock, |_, now| Ok(now.date_naive()))).await;
        assert_eq!(dated, Ok(day("2026-10-01")), "waiting for the lock");
        writer.join().unwrap();
    }

    /// With no data-api configured the provider is CoinGecko, which this build does not fetch from: a walk that needs a
    /// price it lacks is refused with the reason, and one that needs none walks.
    #[tokio::test(flavor = "current_thread")]
    async fn an_unported_price_provider_refuses_the_walk_that_needs_it() {
        let clock = FixedClock(at("2026-10-01T03:00:00Z"));
        let out = with_walk(&install(&SOLD_FIRST), None::<&DataApi<ScriptedTransport>>, 1, &clock, &mut Allowance::run(), |_, _, _, _, _| Ok(())).await;
        assert_eq!(out, Err("the tracker walk is not ported for historical prices from CoinGecko".to_string()));
        let bought = [(4, "USD", "1000", None, "2026-09-01 14:00:00"), (0, "AAPL", "2", Some("400"), "2026-09-02 14:30:00")];
        assert_eq!(with_walk(&install(&bought), None::<&DataApi<ScriptedTransport>>, 1, &clock, &mut Allowance::run(), |_, _, _, _, _| Ok(())).await, Ok(()));
    }

    /// 120 coin fees on 120 days, each day's price arriving only with its own fetch: a run spends at most `MAX_FETCHES`
    /// fetches and stops with the reason, keeping what it stored, and the next runs finish the walk.
    #[tokio::test(flavor = "current_thread")]
    async fn a_run_spends_a_bounded_allowance_and_the_next_run_goes_on() {
        let days: Vec<NaiveDate> = (0..120).map(|i| day("2026-05-01") + chrono::Days::new(i)).collect();
        let mut rows = vec![(0, "BTC".to_string(), "1".to_string(), Some("60000"), "2026-04-30 00:00:00".to_string())];
        rows.extend(days.iter().map(|d| (10, "BTC".to_string(), "0.0001".to_string(), None, format!("{d} 12:00:00"))));
        let rows: Vec<(i64, &str, &str, Option<&str>, &str)> = rows.iter().map(|(k, b, a, u, w)| (*k, b.as_str(), a.as_str(), *u, w.as_str())).collect();
        let db = install(&rows);
        let mut answers = vec![prices(&[])]; // the prefetch's range
        answers.extend(days.iter().map(|d| prices(&[(*d, 60000.0)])));
        let (api, t) = data_api(answers);
        let clock = FixedClock(at("2026-10-01T03:00:00Z"));
        let mut first = Allowance::run();
        let out = with_walk(&db, Some(&api), 1, &clock, &mut first, |_, _, _, _, _| Ok(())).await;
        assert_eq!(out, Err(FETCHES_SPENT.to_string()));
        assert_eq!((t.requests().len(), first.fetches), (1 + MAX_FETCHES, 0));
        assert!(first.steps > WALK.steps - WALK.steps / 100, "{} steps spent", WALK.steps - first.steps);
        let stored: i64 = db.run(|c, _| c.query_row("SELECT count(*) FROM historical_prices", [], |r| r.get(0)).map_err(|e| e.to_string())).await.unwrap();
        assert_eq!(stored, MAX_FETCHES as i64);
        let mut runs = 1;
        loop {
            runs += 1;
            if with_walk(&db, Some(&api), 1, &clock, &mut Allowance::run(), |_, _, _, _, _| Ok(())).await.is_ok() { break; }
            assert!(runs < 3, "a run that fetched nothing new");
        }
        assert_eq!(runs, 3, "120 days: 50, then 51, then the last 19");
    }
}
