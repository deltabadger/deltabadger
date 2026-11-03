//! PLAN-A17 A17-3, Rust only: what the tracker's jobs ask a CoinGecko feed per run (request counts, D-CG-429-STOPS-PRICES
//! as the job's outcome), and a provider switch between two runs taking effect with no restart (jobs::resolve's per-run
//! feed). The values are held to Rails by tracker_parity's and tax_parity's CoinGecko scenarios.
mod common;
mod support { pub mod tracker_fx; }
use deltabadger::engine::FixedClock;
use deltabadger::jobs::{coingecko::{CoinGecko, Plan, Throttle}, data_api::{DataApi, RateCache}, market::Feed, resolve, Cx, Outcome, Wakers};
use deltabadger::venue::http::ScriptedTransport;
use serde_json::{json, Value};
use std::rc::Rc;
use std::sync::Arc;
use support::tracker_fx::{alpaca, at, insert_row, job, ok, Fx, Venues};

/// `alpaca()` with a bitcoin fee on 10 September the walk prices (one lookup).
fn with_fee() -> Fx {
    let fx = alpaca();
    insert_row(&fx.c(), fx.owner(), fx.s.exchange_id, 10, "BTC", "0.0001", None, "2026-09-10 00:00:00");
    fx
}

const RANGE: &str = "GET /api/v3/coins/bitcoin/market_chart/range";

fn days(price: f64) -> Value {
    let day = |d: u32| chrono::NaiveDate::from_ymd_opt(2026, 9, d).unwrap().and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp_millis();
    ok(json!({ "prices": (1..=30).map(|d| json!([day(d), price])).collect::<Vec<_>>() }))
}

fn coingecko(t: &ScriptedTransport, plan: Option<Plan>) -> Feed<ScriptedTransport> {
    let cg = CoinGecko::new("CG-test-key", t.clone()).with_throttle(Arc::new(Throttle::new(usize::MAX)));
    Feed::CoinGecko(match plan { Some(p) => cg.with_plan(p), None => cg })
}

async fn run(fx: &Fx, feed: Feed<ScriptedTransport>) -> Outcome {
    job(fx.owner(), Rc::new(Some(feed))).run(Cx { db: fx.db(), clock: &FixedClock(at()), wakers: Wakers::default() }, vec![]).await
}

fn stored(fx: &Fx) -> i64 { fx.c().query_row("SELECT count(*) FROM historical_prices", [], |r| r.get(0)).unwrap() }

/// One ping (the plan unknown) and one range for the walk's one lookup; the run is Done.
#[tokio::test(flavor = "current_thread")]
async fn a_walk_asks_the_ping_once_and_one_range_per_lookup() {
    let fx = with_fee();
    let t = ScriptedTransport::from_script(&json!({ "GET /api/v3/ping": [ok(json!({}))], RANGE: [days(60000.0)] }));
    assert_eq!(run(&fx, coingecko(&t, None)).await, Outcome::Done);
    let asked: Vec<String> = t.requests().iter().map(|r| r.path.clone()).collect();
    assert_eq!(asked, ["/api/v3/ping", "/api/v3/coins/bitcoin/market_chart/range"]);
    assert!(stored(&fx) > 0);
}

/// D-CG-429-STOPS-PRICES: the range's 429 is the run's last request; the walk still publishes (the fee unpriced) and the
/// job records RateLimited. The next run asks again.
#[tokio::test(flavor = "current_thread")]
async fn a_429_is_the_runs_last_request_and_its_outcome() {
    let fx = with_fee();
    let t = ScriptedTransport::from_script(&json!({ RANGE: [{ "status": 429, "body": {} }] }));
    let out = run(&fx, coingecko(&t, Some(Plan::Demo))).await;
    assert_eq!(out, Outcome::RateLimited(deltabadger::tracker::jobs::RATE_LIMITED.into()));
    assert_eq!(t.requests().len(), 1);
    assert_eq!(stored(&fx), 0);
    let (_, cache_at) = fx.cache();
    assert!(!cache_at.is_empty(), "the walk published");
    let t = ScriptedTransport::from_script(&json!({ RANGE: [days(60000.0)] }));
    assert_eq!(run(&fx, coingecko(&t, Some(Plan::Demo))).await, Outcome::Done);
    assert_eq!(t.requests().len(), 1);
    assert!(stored(&fx) > 0);
}

/// The scheduler's per-run feed (jobs::resolve::configured): a switch from data-api to CoinGecko in app_configs between
/// two runs of one registered job is followed by the second run, with no restart.
#[tokio::test(flavor = "current_thread")]
async fn a_provider_switch_takes_effect_at_the_next_run() {
    let fx = with_fee();
    let cipher = common::seed::cipher();
    let set = |key: &str, value: &str| deltabadger::app_config::set(&fx.c(), &cipher, key, value, at()).unwrap();
    set("market_data_provider", "deltabadger");
    set("market_data_url", "http://data-api:3000");
    set("market_data_token", "t");
    let data_api = ScriptedTransport::from_script(&json!({ "GET /api/v1/historical_prices": [days(60000.0)] }));
    let cg = ScriptedTransport::from_script(&json!({ RANGE: [days(61000.0)] }));
    let (d, g) = (data_api.clone(), cg.clone());
    let feeds: resolve::Feeds<ScriptedTransport> = Rc::new(move |settings, _| Feed::from_settings(settings,
        |config| DataApi::new(config, d.clone(), d.clone()), |key| CoinGecko::new(key, g.clone()).with_throttle(Arc::new(Throttle::new(usize::MAX))).with_plan(Plan::Demo)));
    let wrapped = resolve::configured_feeds(job(fx.owner(), Rc::new(None)), &|_| None, Arc::new(RateCache::default()), Venues, feeds);
    let clock = FixedClock(at());
    let cx = || Cx { db: fx.db(), clock: &clock, wakers: Wakers::default() };
    assert_eq!(wrapped.run(cx(), vec![]).await, Outcome::Done);
    assert_eq!((data_api.requests().len(), cg.requests().len()), (1, 0));
    set("market_data_provider", "coingecko");
    set("coingecko_api_key", "CG-test-key");
    // A day the first run left unpriced, so the second has a lookup to make.
    fx.exec("DELETE FROM historical_prices");
    assert_eq!(wrapped.run(cx(), vec![]).await, Outcome::Done);
    assert_eq!((data_api.requests().len(), cg.requests().len()), (1, 1));
    assert_eq!(cg.requests()[0].path, "/api/v3/coins/bitcoin/market_chart/range");
}
