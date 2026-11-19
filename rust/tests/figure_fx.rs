//! The bots page's conversions: held per provider (`figures::page_market::FxStore`), fetched by the fill and never by a
//! page, and kept figures shown and replayed only while every conversion they read still holds.
//! Wall-clock timing tests here are `#[ignore = "timing: nightly"]`, out of the PR gate; the nightly workflow runs them.
//! By hand: `cargo nextest run --profile timing --run-ignored only` (rust/.config/nextest.toml lists them).
mod common;
use deltabadger::figures::page_market::HeldRate;
use deltabadger::web::figure::loading::{self, Snapshot, Source};
use deltabadger::web::{App, Config};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const AT: &str = "2026-03-18T12:03:30Z";

fn bars(symbol: &str) -> Value {
    let start = common::web::at("2026-03-02T00:00:00Z");
    let rows: Vec<Value> = (0..16).map(|i| {
        let t = (start + chrono::Duration::days(i)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
        json!({"t": t, "o": 100 + i, "h": 100 + i, "l": 100 + i, "c": 100 + i, "v": 1000})
    }).collect();
    if symbol.contains('/') { json!({"bars": {symbol: rows}}) } else { json!({"bars": rows}) }
}

/// AAA in dollars and BTC in tether on Alpaca; the hosted provider's rates and tether's price, `rates` answering 503
/// when false. `tether`: its price as the provider's JSON carries it.
fn script(rates: bool, tether: Value) -> Value {
    let table = json!({"data": {"btc": {"value": 1}, "usd": {"value": 64000.0}, "eur": {"value": 55000.0}}});
    json!({
        "GET data.alpaca.markets/v2/stocks/snapshots": {"body": {"AAA": {"latestTrade": {"p": 110.0}}}},
        "GET data.alpaca.markets/v2/stocks/AAA/bars": {"body": bars("AAA")},
        "GET data.alpaca.markets/v2/stocks/AAA/bars?adjustment=split": {"body": bars("AAA")},
        "GET data.alpaca.markets/v1beta3/crypto/us/latest/trades": {"body": {"trades": {"BTC/USDT": {"p": 120.0}}}},
        "GET data.alpaca.markets/v1beta3/crypto/us/bars?symbols=BTC/USDT": {"body": bars("BTC/USDT")},
        "GET data-api:3000/api/v1/exchange_rates": if rates { json!({"body": table}) } else { json!({"status": 503, "body": {}}) },
        "GET data-api:3000/api/v1/prices?coin_ids=tether&vs_currencies=usd": {"body": {"data": {"tether": {"usd": tether}}}},
    })
}

/// A test clock that counts its readings and, once `armed` with a reading's number, holds that reading until told: a
/// test can then change what a publication checks after it rendered and before it is accepted.
type Armed = Option<(usize, std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>;
struct Gate { clock: Arc<common::web::TestClock>, reads: std::sync::atomic::AtomicUsize, armed: std::sync::Mutex<Armed> }
impl deltabadger::engine::Clock for Gate {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        let n = self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let held = { let mut armed = self.armed.lock().unwrap(); if armed.as_ref().is_some_and(|(at, ..)| *at == n) { armed.take() } else { None } };
        if let Some((_, read, go)) = held { let _ = read.send(()); let _ = go.recv(); }
        deltabadger::engine::Clock::now(&*self.clock)
    }
}
struct Install { _dir: tempfile::TempDir, file: PathBuf, clock: Arc<common::web::TestClock>, gate: Arc<Gate>, app: App }
/// One account with a paper Alpaca key, on the hosted provider, shown in `currency`: a basket of AAA bought for dollars
/// and one of BTC bought for tether.
fn install(currency: &str, script: Value) -> Install { install_with(currency, Source::Script(script)) }
fn install_with(currency: &str, source: Source) -> Install {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("production.sqlite3");
    let c = rusqlite::Connection::open(&file).unwrap();
    c.execute_batch(include_str!("fixtures/settings_primary_schema.sql")).unwrap();
    deltabadger::store::test_support::migrate(&c).unwrap(); // as run/serve take an install
    c.execute_batch("PRAGMA journal_mode=WAL").unwrap();
    let t = "'2026-01-01'";
    c.execute_batch(&format!("
        INSERT INTO exchanges(id,type,name,created_at,updated_at) VALUES (1,'Exchanges::Alpaca','Alpaca',{t},{t});
        INSERT INTO assets(id,category,external_id,symbol,name,created_at,updated_at) VALUES (1,'Fiat','usd','USD','US Dollar',{t},{t}),(2,'Stock','aaa','AAA','AAA Inc',{t},{t}),
            (3,'Cryptocurrency','tether','USDT','Tether',{t},{t}),(4,'Cryptocurrency','bitcoin','BTC','Bitcoin',{t},{t});
        INSERT INTO tickers(id,base,base_asset_id,base_decimals,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,created_at,updated_at)
            VALUES (1,'AAA',2,9,1,0,1,2,'USD',1,2,'AAA',{t},{t}),(2,'BTC',4,9,1,0,1,2,'USDT',3,2,'BTC/USDT',{t},{t});
        INSERT INTO app_configs(key,value,created_at,updated_at) VALUES ('market_data_provider','deltabadger',{t},{t}),('market_data_url','http://data-api:3000',{t},{t}),('market_data_token','token',{t},{t});
        INSERT INTO users(id,email,encrypted_password,setup_completed,admin,confirmed_at,display_currency,created_at,updated_at) VALUES (1,'u1@example.com','x',1,1,{t},'{currency}',{t},{t});
        INSERT INTO bots(id,user_id,exchange_id,type,status,settings,transient_data,created_at,updated_at) VALUES
            (1,1,1,'Bots::DcaMultiAsset',1,'{{\"quote_asset_id\":1,\"interval\":\"day\",\"quote_amount\":100,\"allocations\":{{\"2\":1.0}}}}','{{}}',{t},{t}),
            (2,1,1,'Bots::DcaMultiAsset',1,'{{\"quote_asset_id\":3,\"interval\":\"day\",\"quote_amount\":100,\"allocations\":{{\"4\":1.0}}}}','{{}}',{t},{t});
        INSERT INTO transactions(bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) VALUES
            (1,1,'AAA',2,'USD',1,1,1,100,100,0,0,2,'o1','REGULAR','2026-03-02 14:30:00',{t}),
            (2,1,'BTC',4,'USDT',3,1,1,100,100,0,0,2,'o2','REGULAR','2026-03-02 14:30:00',{t});
    ")).unwrap();
    let clock = common::web::TestClock::at(AT);
    let gate = Arc::new(Gate { clock: clock.clone(), reads: Default::default(), armed: Default::default() });
    let env = common::web::env(common::web::SECRET);
    let app = App::new(Config::from_env(&env).unwrap(), &env, rusqlite::Connection::open(&file).unwrap(), gate.clone()).unwrap()
        .with_figure_source(source).unwrap();
    c.execute(&format!("INSERT INTO api_keys(user_id,exchange_id,key_type,status,key,secret,passphrase,created_at,updated_at) VALUES (1,1,0,1,?1,?2,?3,{t},{t})"),
              rusqlite::params![app.cipher.encrypt("key"), app.cipher.encrypt("secret"), app.cipher.encrypt("paper")]).unwrap();
    Install { _dir: dir, file, clock, gate, app }
}
impl Install {
    /// The `figures` service of `serve`, with no engine.
    fn consume(&self) -> tokio::sync::watch::Sender<bool> {
        let (events, rx) = tokio::sync::mpsc::unbounded_channel();
        let (stop, stopped) = tokio::sync::watch::channel(false);
        std::mem::forget(events);
        tokio::spawn(loading::follow(self.app.clone(), rx, stopped));
        stop
    }
    fn identity(&self) -> String { deltabadger::web::tracker::fx::provider(&rusqlite::Connection::open(&self.file).unwrap()).unwrap().0 }
    fn set(&self, time: &str) { self.clock.set(common::web::at(time)); }
    /// What the dashboard shows now; a page that finds nothing kept asks the queue for it.
    async fn page(&self) -> Option<Value> {
        let snapshot = loading::prepare(&self.app, 1).await.unwrap();
        let app = self.app.clone();
        self.app.db(move |c| loading::render_page(&app, c, 1, &snapshot, "en", "token", "")).await.unwrap()
    }
    /// The dashboard, once it shows figures.
    async fn shown(&self, what: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(v) = self.page().await { return v; }
            assert!(Instant::now() < deadline, "{what}: no figures within 20 s");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
fn tile(v: &Value, bot: i64) -> String { v["bots"][bot.to_string()]["tile"].as_str().unwrap().to_string() }
fn headline(v: &Value) -> String { v["account"].as_str().unwrap().to_string() }

#[tokio::test]
async fn tether_and_euro_figures_are_converted_by_the_fill() {
    let install = install("EUR", script(true, json!(0.9993)));
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    // 1 USD = 55000/64000 EUR; the tether basket's profit is converted to dollars at 0.9993 first.
    assert!(tile(&shown, 1).contains("<small>€</small>8.59"), "{}", tile(&shown, 1));
    assert!(tile(&shown, 2).contains("<small>€</small>17.18"), "{}", tile(&shown, 2));
    let head = headline(&shown);
    assert!(head.contains("<small>€</small>26") && head.contains("data-pnl-spark-unit-value=\"€\"") && head.contains("data-pnl-spark-rate-value=\"0.859375\""), "{head}");
    // Both conversions are held for the provider, success for twelve hours (fiat and a stablecoin).
    let store = &install.app.fx_cache.store;
    let now = common::web::at(AT).timestamp();
    for (from, to) in [("USD", "EUR"), ("USDT", "USD")] {
        let held = store.get(&install.identity(), from, to, now).unwrap_or_else(|| panic!("{from}->{to} not held"));
        assert!(held.value.is_ok() && held.until >= now + 12 * 3600 - 60, "{from}->{to}: {held:?}");
    }
}

#[tokio::test]
async fn a_page_never_fetches_a_conversion() {
    // Every answer is held back five seconds: the fill a page starts is still waiting on the network while the page reads.
    let mut held = script(true, json!(0.9993));
    for (_, reply) in held.as_object_mut().unwrap().iter_mut() { reply["delay_ms"] = json!(5000); }
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let install = install_with("EUR", Source::Counted(held, log.clone()));
    let started = Instant::now();
    for _ in 0..3 { assert!(install.page().await.is_none(), "a page showed figures before any answer"); }
    assert!(started.elapsed() < Duration::from_secs(2), "a page waited on the network: {:?}", started.elapsed());
    let now = common::web::at(AT).timestamp();
    for (from, to) in [("USD", "EUR"), ("USDT", "USD")] { assert!(install.app.fx_cache.store.get(&install.identity(), from, to, now).is_none(), "{from}->{to} answered while held back"); }
    // The one fill the pages started asks the provider once for each conversion, however many pages asked.
    let deadline = Instant::now() + Duration::from_secs(20);
    while install.app.fx_cache.store.get(&install.identity(), "USDT", "USD", now).is_none() {
        assert!(Instant::now() < deadline, "the fill never held the conversions");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let asked = log.lock().unwrap().clone();
    let count = |key: &str| asked.iter().filter(|k| k.as_str() == key).count();
    assert_eq!(count("GET data-api:3000/api/v1/exchange_rates"), 1, "{asked:?}");
    assert_eq!(count("GET data-api:3000/api/v1/prices?coin_ids=tether&vs_currencies=usd"), 1, "{asked:?}");
}

#[tokio::test]
async fn a_quote_rate_refreshed_at_the_same_revision_renders_the_figures_anew() {
    let install = install("USD", script(true, json!(0.9993)));
    let _stop = install.consume();
    let before = install.shown("ready").await;
    assert!(tile(&before, 2).contains("<small>$</small>19.99"), "{}", tile(&before, 2));
    // The tether rate is refreshed (a new answer for the same pair); the database and the display rate are as they were.
    let now = common::web::at(AT).timestamp();
    install.app.fx_cache.store.put(&install.identity(), "USDT", "USD", Ok(HeldRate::Decimal("0.5".into())), now + 3600, now);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(v) = install.page().await { if tile(&v, 2).contains("<small>$</small>10.00") { break; } assert_ne!(tile(&v, 2), "", "stale figures served"); }
        assert!(Instant::now() < deadline, "the refreshed rate never reached the tile");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_held_fallback_expiring_between_cuts_misses_and_recovers() {
    let install = install("EUR", script(false, json!(0.9993)));
    let _stop = install.consume();
    // No rate table: dollars, as Denomination.for falls back. The failure is held five minutes, to 12:08:30.
    let first = install.shown("fallback").await;
    assert!(headline(&first).contains("<small>$</small>") && headline(&first).contains("data-pnl-spark-rate-value=\"1.0\""), "{}", headline(&first));
    // After the 12:05 cut the figures are refilled, still reading the held failure.
    install.set("2026-03-18T12:06:00Z");
    let refilled = install.shown("refilled").await;
    assert!(headline(&refilled).contains("<small>$</small>"));
    // 12:08:31, before the 12:10 cut: the failure has expired, so the figures that read it are not shown.
    install.set("2026-03-18T12:08:31Z");
    let snapshot = loading::prepare(&install.app, 1).await.unwrap();
    assert!(matches!(snapshot, Snapshot::Ready(..)), "the market cache is still within its cut");
    let clock = install.app.clock.clone();
    let shown = install.app.db(move |c| loading::render(c, 1, &snapshot, "en", "token", "", &*clock)).await.unwrap();
    assert!(shown.is_none(), "figures rendered with an expired fallback were shown");
    // The queue asks the provider again (held anew) and renders them.
    let recovered = install.shown("recovered").await;
    assert!(headline(&recovered).contains("<small>$</small>"));
    let held = install.app.fx_cache.store.get(&install.identity(), "USD", "EUR", common::web::at("2026-03-18T12:08:31Z").timestamp()).expect("asked again");
    assert!(held.value.is_err() && held.until > common::web::at("2026-03-18T12:08:31Z").timestamp());
}

#[tokio::test]
async fn a_reconnect_is_not_replayed_figures_whose_rate_was_refreshed() {
    let install = install("EUR", script(true, json!(0.9993)));
    let _stop = install.consume();
    install.shown("ready").await;
    let stream = "user_1:bot_updates";
    // Publish, as the end of a fill does for a waiting page.
    loading::publish(&install.app, 1).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while loading::latest(&install.app, 1, stream).await.is_none_or(|kept| kept.is_empty()) {
        assert!(Instant::now() < deadline, "never published");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!loading::resubscribed(&install.app, 1, stream).await.is_empty(), "a fresh publication is replayed");
    // A tracker request refreshes the display rate; the revision is unchanged.
    let now = common::web::at(AT).timestamp();
    install.app.fx_cache.store.put(&install.identity(), "USD", "EUR", Ok(HeldRate::Float(0.5)), now + 3600, now);
    assert!(loading::resubscribed(&install.app, 1, stream).await.is_empty(), "figures in the old rate were replayed");
}

/// A listed divergence, not parity: the provider sent tether's price as a String. 2.x's User#global_pnl multiplies by
/// it and raises (`String can't be coerced`), a 2.x defect on a display-only figure; the server reads the decimal it
/// spells and shows the total.
#[tokio::test]
async fn divergence_a_string_coin_price_still_shows_the_total() {
    let install = install("USD", script(true, json!("0.9993")));
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    assert!(headline(&shown).contains("pnl-amount"), "{}", headline(&shown));
    assert!(tile(&shown, 2).contains("<small>$</small>19.99"), "{}", tile(&shown, 2));
}

#[tokio::test]
#[ignore = "timing: nightly"]
async fn a_rate_refreshed_between_render_and_acceptance_refuses_the_publication() {
    let install = install("EUR", script(true, json!(0.9993)));
    let stop = install.consume();
    install.shown("ready").await;
    stop.send(true).unwrap(); // nothing else reads the clock from here on
    tokio::task::yield_now().await;
    let stream = "user_1:bot_updates";
    // A publication as it runs unhindered: how many readings of the clock it takes; the last is its acceptance check.
    let before = install.gate.reads.load(std::sync::atomic::Ordering::SeqCst);
    loading::publish(&install.app, 1).await.unwrap();
    let taken = install.gate.reads.load(std::sync::atomic::Ordering::SeqCst) - before;
    assert!(loading::latest(&install.app, 1, stream).await.is_some_and(|kept| !kept.is_empty()), "the control publication was accepted");
    // The same publication again, held at that reading: it has rendered and checked the figures; the display rate is
    // refreshed now, and the publication must not be accepted.
    let (read, arrived) = std::sync::mpsc::channel();
    let (resume, go) = std::sync::mpsc::channel();
    let at = install.gate.reads.load(std::sync::atomic::Ordering::SeqCst) + taken;
    *install.gate.armed.lock().unwrap() = Some((at, read, go));
    let (store, identity) = (install.app.fx_cache.store.clone(), install.identity());
    let refresher = std::thread::spawn(move || {
        arrived.recv().unwrap();
        let now = common::web::at(AT).timestamp();
        store.put(&identity, "USD", "EUR", Ok(HeldRate::Float(0.5)), now + 3600, now);
        resume.send(()).unwrap();
    });
    loading::publish(&install.app, 1).await.unwrap();
    refresher.join().unwrap();
    assert!(install.gate.armed.lock().unwrap().is_none(), "the publication never reached its acceptance");
    assert!(loading::latest(&install.app, 1, stream).await.is_none(), "a publication in the old rate was accepted");
}

#[test]
fn only_alpaca_bots_name_the_symbols_alpaca_is_asked_for() {
    let install = install("USD", script(true, json!(1.0)));
    let c = rusqlite::Connection::open(&install.file).unwrap();
    let t = "'2026-01-01'";
    // KuCoin writes its pairs `BTC-USDC`: an invalid symbol at Alpaca, which then fails the whole request.
    c.execute_batch(&format!("
        INSERT INTO exchanges(id,type,name,created_at,updated_at) VALUES (2,'Exchanges::Kucoin','KuCoin',{t},{t});
        INSERT INTO assets(id,category,external_id,symbol,name,created_at,updated_at) VALUES (5,'Cryptocurrency','usd-coin','USDC','USDC',{t},{t});
        INSERT INTO tickers(id,base,base_asset_id,base_decimals,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,available,trading_enabled,created_at,updated_at)
            VALUES (3,'BTC',4,8,2,0,1,2,'USDC',5,2,'BTC-USDC',1,1,{t},{t});
        INSERT INTO bots(id,user_id,exchange_id,type,status,settings,transient_data,created_at,updated_at) VALUES
            (3,1,2,'Bots::DcaMultiAsset',1,'{{\"quote_asset_id\":5,\"allocations\":{{\"4\":1.0}}}}','{{}}',{t},{t});
    ")).unwrap();
    assert_eq!(loading::symbols(&c, 1).unwrap(), vec!["AAA".to_string(), "BTC/USDT".to_string()]);
}

#[tokio::test]
async fn a_bot_on_a_venue_the_figures_cannot_read_leaves_the_others_figures() {
    let install = install("EUR", script(true, json!(0.9993)));
    let c = rusqlite::Connection::open(&install.file).unwrap();
    let t = "'2026-01-01'";
    // A KuCoin basket with an order: its chart asks for candles the reader has no venue for.
    c.execute_batch(&format!("
        INSERT INTO exchanges(id,type,name,created_at,updated_at) VALUES (2,'Exchanges::Bitget','Bitget',{t},{t});
        INSERT INTO assets(id,category,external_id,symbol,name,created_at,updated_at) VALUES (5,'Cryptocurrency','usd-coin','USDC','USDC',{t},{t});
        INSERT INTO tickers(id,base,base_asset_id,base_decimals,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,available,trading_enabled,created_at,updated_at)
            VALUES (3,'BTC',4,8,2,0,1,2,'USDC',5,2,'BTCUSDC',1,1,{t},{t});
        INSERT INTO bots(id,user_id,exchange_id,type,status,settings,transient_data,created_at,updated_at) VALUES
            (3,1,2,'Bots::DcaMultiAsset',1,'{{\"quote_asset_id\":5,\"allocations\":{{\"4\":1.0}}}}','{{}}',{t},{t});
        INSERT INTO transactions(bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) VALUES
            (3,2,'BTC',4,'USDC',5,1,1,100,100,0,0,2,'o3','REGULAR','2026-03-02 14:30:00',{t});
    ")).unwrap();
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    assert!(tile(&shown, 1).contains("<small>€</small>8.59"), "{}", tile(&shown, 1));
    assert!(tile(&shown, 2).contains("<small>€</small>17.18"), "{}", tile(&shown, 2));
}

#[tokio::test]
async fn a_raised_price_read_leaves_only_its_bot_without_figures_and_holds_the_headline() {
    // The crypto quote raises (a transient network failure): the tether basket's figures stay loading, as 2.x's metrics job for
    // that bot raises and is retried. The dollar basket keeps its figures, and the headline waits while the traded bot has none.
    let mut sc = script(true, json!(0.9993));
    sc["GET data.alpaca.markets/v1beta3/crypto/us/latest/trades"] = json!({"network": "transient", "message": "Faraday::ConnectionFailed: connection refused"});
    let install = install("EUR", sc);
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    assert!(tile(&shown, 1).contains("<small>€</small>8.59"), "{}", tile(&shown, 1));
    assert_eq!(shown["bots"]["2"]["metrics"], "<div id=\"metrics\"></div>", "the bot whose prices raised shows no figures");
    assert_eq!(shown["bots"]["2"]["tile"], "<div id=\"pnl_bots_dca_multi_asset_2\"></div>");
    assert_eq!(shown["account"], "<div id=\"global-pnl\"></div>", "no total while a traded bot has no figures");
    // They last only the price's minute: the next read fills again and asks the quote afresh.
    let loading::Snapshot::Ready(.., until) = loading::prepare(&install.app, 1).await.unwrap() else { panic!("not ready") };
    assert!(until <= common::web::at(AT).timestamp() + 60, "figures with a raised bot outlived the price's minute: {until}");
}

/// A Gemini basket of BTC for USDC (bot 3) that only sold: 0.002 for 5 (order 3) and 0.001 for 2.6 (order 4), beside the
/// install's buyers. Its venue (Gemini, read since A13 through its price feed) answers, so there is no note that it did not.
fn gemini_seller(signal: bool) -> Install {
    let mut sc = script(true, json!(1.0));
    sc["GET api.gemini.com/v1/pricefeed"] = json!({"body": [{"pair": "BTCUSDC", "price": "2600", "percentChange24h": "0.0"}, {"pair": "BTCEUR", "price": "90", "percentChange24h": "0.0"}]});
    let install = install("USD", sc);
    let c = rusqlite::Connection::open(&install.file).unwrap();
    let t = "'2026-01-01'";
    let (class, settings) = if signal { ("Bots::Signal", r#"{"base_asset_id":4,"quote_asset_id":5}"#) } else { ("Bots::DcaMultiAsset", r#"{"quote_asset_id":5,"allocations":{"4":1.0},"direction":"selling"}"#) };
    c.execute_batch(&format!("
        INSERT INTO exchanges(id,type,name,created_at,updated_at) VALUES (2,'Exchanges::Gemini','Gemini',{t},{t});
        INSERT INTO assets(id,category,external_id,symbol,name,created_at,updated_at) VALUES (5,'Cryptocurrency','usd-coin','USDC','USDC',{t},{t}),(6,'Fiat','eur','EUR','Euro',{t},{t});
        INSERT INTO tickers(id,base,base_asset_id,base_decimals,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,available,trading_enabled,created_at,updated_at)
            VALUES (3,'BTC',4,8,2,0,1,2,'USDC',5,2,'btcusdc',1,1,{t},{t}),(4,'BTC',4,8,2,0,1,2,'EUR',6,2,'btceur',1,1,{t},{t});
        INSERT INTO bots(id,user_id,exchange_id,type,status,settings,transient_data,created_at,updated_at) VALUES (3,1,2,'{class}',1,'{settings}','{{}}',{t},{t});
        INSERT INTO transactions(id,bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) VALUES
            (3,3,2,'BTC',4,'USDC',5,0.002,0.002,2500,5,1,0,2,'o3','REGULAR','2026-03-02 14:30:00',{t}),
            (4,3,2,'BTC',4,'USDC',5,0.001,0.001,2600,2.6,1,0,2,'o4','REGULAR','2026-03-03 14:30:00',{t});
    ")).unwrap();
    install
}
/// A row of the account's ledger on `venue`: `kind` as stored (0 buy, 1 sell, 4 deposit, 5 withdrawal), the bot order it
/// is a fill of, and the row it is linked to.
#[allow(clippy::too_many_arguments)]
fn ledger(c: &rusqlite::Connection, id: i64, venue: i64, kind: i64, amount: &str, quote: Option<(&str, &str)>, fee: Option<(&str, &str)>, at: &str, order: Option<i64>, linked: Option<i64>) {
    c.execute("INSERT INTO account_transactions(id,user_id,exchange_id,entry_type,base_currency,base_asset_id,base_amount,quote_currency,quote_amount,fee_currency,fee_amount,tx_id,raw_data,transacted_at,transaction_id,linked_transaction_id,created_at,updated_at) \
               VALUES (?1,1,?2,?3,'BTC',4,?4,?5,?6,?7,?8,?9,'{}',?10,?11,?12,'2026-03-17 00:00:00','2026-03-17 00:00:00')",
              rusqlite::params![id, venue, kind, amount, quote.map(|q| q.0), quote.map(|q| q.1), fee.map(|f| f.0), fee.map(|f| f.1), format!("t{id}"), at, order, linked]).unwrap();
}
/// The tracker job's walk of the ledger, published as the job publishes it (`tracker::cache::publish`).
fn walk_and_publish(install: &Install) -> deltabadger::tracker::walk::Walked {
    use deltabadger::tracker::{jobs, prices::{PriceService, Unfetched}, walk};
    let c = rusqlite::Connection::open(&install.file).unwrap();
    let inputs = jobs::load_inputs(&c, 1).unwrap();
    let mut none = Unfetched;
    let mut prices = PriceService::new(&inputs.reference, &inputs.fx, common::web::at(AT).date_naive(), &mut none);
    let walked = walk::scopes(&inputs.rows, &inputs.deposits, &mut prices, deltabadger::figures::at::At::from_utc(common::web::at(AT)).unwrap()).unwrap();
    let tx = deltabadger::engine::model::immediate(&c).unwrap();
    let fenced = deltabadger::engine::model::fence_versions(&tx, &[]).unwrap();
    // The account's Alpaca key made its venue's ledger import, as a sync records it: the ledger is current.
    let key = deltabadger::engine::model::credential_version_by_id(&fenced, 1).unwrap().unwrap();
    deltabadger::sync::cache::record_ledger(&fenced, 1, &key, common::web::at(AT)).unwrap();
    assert!(deltabadger::tracker::cache::publish(&fenced, 1, &deltabadger::tracker::cache::version(&fenced, 1).unwrap(), Some(&walked), common::web::at(AT)).unwrap());
    tx.commit().unwrap();
    walked
}
/// The seller's two orders, filled from 0.003 BTC bought for 6 USDC on its venue.
fn sold_from_a_purchase(install: &Install) {
    let c = rusqlite::Connection::open(&install.file).unwrap();
    ledger(&c, 1, 2, 0, "0.003", Some(("USDC", "6")), None, "2026-03-01 10:00:00", None, None);
    ledger(&c, 2, 2, 1, "0.002", Some(("USDC", "5")), None, "2026-03-02 14:30:00", Some(3), None);
    ledger(&c, 3, 2, 1, "0.001", Some(("USDC", "2.6")), None, "2026-03-03 14:30:00", Some(4), None);
}
fn metrics(v: &Value, bot: i64) -> String { v["bots"][bot.to_string()]["metrics"].as_str().unwrap().to_string() }
/// The bot's figures state no P/L: no box, and a tile without a figure.
fn no_pnl(v: &Value, bot: i64) {
    let metrics = metrics(v, bot);
    assert!(metrics.contains("Received") && !metrics.contains("Realised P/L"), "{metrics}");
    assert!(!tile(v, bot).contains("pnl-percent"), "{}", tile(v, bot));
}

#[tokio::test]
async fn a_sell_only_bot_says_what_it_sold_and_received() {
    // 3.0, Rust only: a bot that only sells (what it sells came from outside) states what it sold and what that brought
    // in, not an investment.
    let install = gemini_seller(false);
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    let metrics = metrics(&shown, 3);
    assert!(metrics.contains(">Sold</div>") && metrics.contains("0.003 <small>BTC</small>"), "{metrics}");
    assert!(metrics.contains(">Received</div>") && metrics.contains("7.60 <small>USDC</small>"), "{metrics}");
    assert!(!metrics.contains("Total invested") && !metrics.contains("could not be read from the exchange"), "{metrics}");
    // No ledger walked: what the sold coins cost is not known, and no P/L is stated (D-SELL-ONLY-KNOWN-COST 1).
    no_pnl(&shown, 3);
    assert!(metrics.contains("data-grid--two-columns"), "{metrics}");
    // A bot that bought keeps its investment.
    assert!(shown["bots"]["1"]["metrics"].as_str().unwrap().contains("Total invested"));
}

/// D-SELL-ONLY-PNL-IS-RECEIVED-MINUS-COST, D-SELL-ONLY-TILE: 7.60 received for coins that cost 6.00.
async fn known_cost_states_the_realised_pnl(signal: bool) {
    let install = gemini_seller(signal);
    sold_from_a_purchase(&install);
    let walked = walk_and_publish(&install);
    assert_eq!(walked.bot_sales.iter().map(|s| (s.order, s.cost.to_s_f(), s.known)).collect::<Vec<_>>(), [(3, "4.0".into(), true), (4, "2.0".into(), true)]);
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    let metrics = metrics(&shown, 3);
    assert!(metrics.contains("Received") && metrics.contains("7.60 <small>USDC</small>"), "{metrics}");
    assert!(metrics.contains("Realised P/L") && metrics.contains("text-success") && metrics.contains("+1.60 <small>USDC</small>"), "{metrics}");
    assert!(metrics.contains("Received minus what the sold coins cost, from your account&#39;s history.") || metrics.contains("Received minus what the sold coins cost, from your account's history."), "{metrics}");
    assert!(!metrics.contains("data-grid--two-columns"), "{metrics}");
    assert!(tile(&shown, 3).contains("26.67%") && tile(&shown, 3).contains("text-success"), "{}", tile(&shown, 3));
}
#[tokio::test]
async fn a_sell_only_bot_with_known_cost_states_its_realised_pnl() { known_cost_states_the_realised_pnl(false).await; }
#[tokio::test]
async fn a_sell_only_pair_with_known_cost_states_its_realised_pnl() { known_cost_states_the_realised_pnl(true).await; }

#[tokio::test]
async fn a_sale_of_an_unlinked_deposit_states_no_pnl() {
    let install = gemini_seller(false);
    let c = rusqlite::Connection::open(&install.file).unwrap();
    // Arrived from outside with no link to where it came from: its cost is assumed, never known (Review focus 1).
    ledger(&c, 1, 2, 4, "0.003", None, None, "2026-03-01 10:00:00", None, None);
    ledger(&c, 2, 2, 1, "0.002", Some(("USDC", "5")), None, "2026-03-02 14:30:00", Some(3), None);
    ledger(&c, 3, 2, 1, "0.001", Some(("USDC", "2.6")), None, "2026-03-03 14:30:00", Some(4), None);
    assert!(walk_and_publish(&install).bot_sales.iter().all(|s| !s.known));
    let _stop = install.consume();
    no_pnl(&install.shown("ready").await, 3);
}

#[tokio::test]
async fn a_fill_the_ledger_has_not_all_of_states_no_pnl() {
    let install = gemini_seller(false);
    let c = rusqlite::Connection::open(&install.file).unwrap();
    ledger(&c, 1, 2, 0, "0.003", Some(("USDC", "6")), None, "2026-03-01 10:00:00", None, None);
    // Order 3 filled 0.002; the ledger has synced 0.0015 of it.
    ledger(&c, 2, 2, 1, "0.0015", Some(("USDC", "3.75")), None, "2026-03-02 14:30:00", Some(3), None);
    ledger(&c, 3, 2, 1, "0.001", Some(("USDC", "2.6")), None, "2026-03-03 14:30:00", Some(4), None);
    walk_and_publish(&install);
    let _stop = install.consume();
    no_pnl(&install.shown("ready").await, 3);
}

#[tokio::test]
async fn an_executed_sale_without_proceeds_withholds_the_pnl() {
    let install = gemini_seller(false);
    sold_from_a_purchase(&install);
    // A liquidation that executed units and reported no proceeds: they left with no fill (Review focus 4).
    rusqlite::Connection::open(&install.file).unwrap().execute_batch("
        INSERT INTO transactions(id,bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) VALUES
            (5,3,2,'BTC',4,'USDC',5,0.0005,0.0005,2600,NULL,1,0,2,'o5','LIQUIDATION','2026-03-04 14:30:00','2026-01-01');").unwrap();
    walk_and_publish(&install);
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    no_pnl(&shown, 3);
    assert!(metrics(&shown, 3).contains("7.60 <small>USDC</small>"), "{}", metrics(&shown, 3));
}

#[tokio::test]
async fn a_cold_ledger_states_no_pnl() {
    let install = gemini_seller(true);
    sold_from_a_purchase(&install);
    let _stop = install.consume();
    no_pnl(&install.shown("ready").await, 3);
}

/// One Binance sell of 1 BTC for 150 USDC (order 3), its commission `fee` in `asset`, read as the venue reader stores the
/// bot's order and as the ledger sync stores the trade, over 1 BTC bought for 100 USDC.
async fn binance_sale(fee: &str, asset: &str) -> Value {
    use deltabadger::venue::binance::{parse_trade, Pair, Pairs};
    let install = gemini_seller(false);
    let c = rusqlite::Connection::open(&install.file).unwrap();
    let trade = json!({"symbol": "BTCUSDC", "id": 1, "orderId": 3, "price": "150", "qty": "1", "quoteQty": "150", "commission": fee, "commissionAsset": asset,
                       "isBuyer": false, "isMaker": false, "time": 1_773_066_600_000_i64});
    let pairs = Pairs { by_symbol: [("BTCUSDC".to_string(), Pair { base: "BTC".into(), quote: "USDC".into(), price_decimals: 2 })].into(), assets: Default::default() };
    let order = parse_trade(&trade, &pairs).unwrap();
    c.execute_batch("DELETE FROM transactions WHERE bot_id=3").unwrap();
    c.execute("INSERT INTO transactions(id,bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) \
               VALUES (3,3,2,'BTC',4,'USDC',5,?1,?2,150,?3,1,0,2,'o3','REGULAR','2026-03-09 14:30:00','2026-01-01')",
              rusqlite::params![order.amount.to_s_f(), order.amount_exec.to_s_f(), order.quote_amount_exec.to_s_f()]).unwrap();
    ledger(&c, 1, 2, 0, "1", Some(("USDC", "100")), None, "2026-03-01 10:00:00", None, None);
    // The fee coin's price that day, as the price backfill stores it: the walk values the fee with it.
    c.execute("INSERT INTO historical_prices(asset,currency,date,price) VALUES (?1,'USD','2026-03-09','600')", [asset]).unwrap();
    let legs = deltabadger::sync::binance_ledger::normalize_trade(&deltabadger::sync::activities::Raw::from_value(trade), "BTCUSDC", "BTC", "USDC").unwrap();
    for (i, leg) in legs.iter().enumerate() {
        c.execute("INSERT INTO account_transactions(id,user_id,exchange_id,entry_type,base_currency,base_asset_id,base_amount,quote_currency,quote_amount,fee_currency,fee_amount,tx_id,group_id,raw_data,transacted_at,transaction_id,created_at,updated_at) \
                   VALUES (?1,1,2,?2,?3,4,?4,?5,?6,?7,?8,?9,?10,'{}',?11,3,'2026-03-17 00:00:00','2026-03-17 00:00:00')",
                  rusqlite::params![2 + i as i64, leg.entry_type, leg.base_currency, leg.base_amount.to_s_f(), leg.quote_currency, leg.quote_amount.as_ref().map(|q| q.to_s_f()),
                                    leg.fee_currency, leg.fee_amount.as_ref().map(|f| f.to_s_f()), leg.tx_id, leg.group_id, leg.transacted_at.unwrap().format("%Y-%m-%d %H:%M:%S").to_string()]).unwrap();
    }
    walk_and_publish(&install);
    let _stop = install.consume();
    install.shown("ready").await
}

#[tokio::test]
async fn a_quote_fee_is_counted_once() {
    // Received is net of the 1.50 the venue kept; the cost is the lot's 100: P/L 48.50, not 50 and not 47 (Review focus 3).
    let shown = binance_sale("1.5", "USDC").await;
    let metrics = metrics(&shown, 3);
    assert!(metrics.contains("148.50 <small>USDC</small>") && metrics.contains("+48.50 <small>USDC</small>"), "{metrics}");
    // A fee in a third coin is not the bot's to subtract: the tracker counts it.
    let metrics = metrics_of(binance_sale("0.01", "BNB").await);
    assert!(metrics.contains("150.00 <small>USDC</small>") && metrics.contains("+50.00 <small>USDC</small>"), "{metrics}");
    // A fee in the coin sold leaves outside the sale: its cost is not in it, and the bot counts those units as sold.
    no_pnl(&binance_sale("0.001", "BTC").await, 3);
}
fn metrics_of(v: Value) -> String { metrics(&v, 3) }

#[tokio::test]
async fn a_euro_bot_states_its_pnl_in_euros() {
    async fn sold_for(currency: &str, paid: &str) -> Value {
        let install = gemini_seller(false);
        let c = rusqlite::Connection::open(&install.file).unwrap();
        c.execute_batch("UPDATE bots SET settings='{\"quote_asset_id\":6,\"allocations\":{\"4\":1.0},\"direction\":\"selling\"}' WHERE id=3;
            DELETE FROM transactions WHERE bot_id=3;
            INSERT INTO transactions(id,bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) VALUES
                (3,3,2,'BTC',4,'EUR',6,1,1,100,100,1,0,2,'o3','REGULAR','2026-03-10 14:30:00','2026-01-01');
            INSERT INTO fx_rates(currency,date,rate) VALUES ('USD','2026-03-10',1.1);").unwrap();
        // A lot that cost 55 dollars, sold for 100 euros worth 110 dollars that day: it cost 50 euros (D-SELL-ONLY-QUOTE-RATE).
        ledger(&c, 1, 2, 0, "1", Some(("USD", "55")), None, "2026-03-01 10:00:00", None, None);
        ledger(&c, 2, 2, 1, "1", Some((currency, paid)), None, "2026-03-10 14:30:00", Some(3), None);
        walk_and_publish(&install);
        let _stop = install.consume();
        install.shown("ready").await
    }
    let metrics = metrics_of(sold_for("EUR", "100").await);
    assert!(metrics.contains("100.00 <small>EUR</small>") && metrics.contains("+50.00 <small>EUR</small>"), "{metrics}");
    // The ledger sold for dollars: not the bot's quote, so no P/L in it.
    no_pnl(&sold_for("USD", "110").await, 3);
}

#[tokio::test]
async fn an_unlinked_transfer_withdraws_the_kept_pnl() {
    let install = gemini_seller(false);
    let c = rusqlite::Connection::open(&install.file).unwrap();
    // Bought on another venue and moved over, the transfer linked: the cost came with the coins.
    c.execute_batch("INSERT INTO exchanges(id,type,name,created_at,updated_at) VALUES (3,'Exchanges::Kraken','Kraken','2026-01-01','2026-01-01')").unwrap();
    ledger(&c, 1, 3, 0, "0.003", Some(("USDC", "6")), None, "2026-03-01 10:00:00", None, None);
    ledger(&c, 2, 3, 5, "0.003", None, None, "2026-03-01 12:00:00", None, Some(3));
    ledger(&c, 3, 2, 4, "0.003", None, None, "2026-03-01 12:00:00", None, None);
    ledger(&c, 4, 2, 1, "0.002", Some(("USDC", "5")), None, "2026-03-02 14:30:00", Some(3), None);
    ledger(&c, 5, 2, 1, "0.001", Some(("USDC", "2.6")), None, "2026-03-03 14:30:00", Some(4), None);
    assert!(walk_and_publish(&install).bot_sales.iter().all(|s| s.known));
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    assert!(metrics(&shown, 3).contains("+1.60 <small>USDC</small>"), "{}", metrics(&shown, 3));
    let Snapshot::Ready(cache, at, revision, mark, until) = loading::prepare(&install.app, 1).await.unwrap() else { panic!("not ready") };
    // The user unlinks the transfer: the ledger moved, and no republish has happened yet (Review focus 2).
    c.execute("UPDATE account_transactions SET linked_transaction_id = NULL, updated_at = '2026-03-18 12:00:00' WHERE id = 2", []).unwrap();
    for (snapshot, stale) in [(Snapshot::Ready(cache.clone(), at, revision, mark, until), false), (Snapshot::Stale(cache, at, revision, until), true)] {
        let clock = install.clock.clone();
        let shown = install.app.db(move |c| loading::render(c, 1, &snapshot, "en", "token", "", &*clock)).await.unwrap();
        // Ready: the revision moves first, so the kept figures are not served. Stale: the bot's kept figures are withheld.
        if stale { assert!(shown.is_none(), "stale figures still served: {shown:?}"); }
        assert!(shown.as_ref().is_none_or(|v| !v.to_string().contains("Realised P/L")), "kept figures still state the P/L: {shown:?}");
    }
    // Republished: what the deposit cost is assumed, and the figures say so.
    assert!(walk_and_publish(&install).bot_sales.iter().all(|s| !s.known));
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(v) = install.page().await.filter(|v| v["bots"]["3"]["metrics"].as_str().is_some_and(|m| m.contains("Received"))) { no_pnl(&v, 3); break; }
        assert!(Instant::now() < deadline, "no figures within 20 s");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The chart's `data-bot--chart-<name>-value`, unescaped; None when the chart has no such attribute.
fn chart_value(v: &Value, bot: i64, name: &str) -> Option<String> {
    let chart = v["bots"][bot.to_string()]["chart"].as_str().unwrap();
    let start = chart.find(&format!("data-bot--chart-{name}-value=\"")).map(|at| at + name.len() + 24)?;
    let raw = &chart[start..start + chart[start..].find('"').unwrap()];
    Some(raw.replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&"))
}
/// The chart's series as its data request answers from them (D-CHART-DATA-REQUEST: they are kept with the record, not in
/// the shell): `series` is [value, invested]; `pnl` their difference, none while Received's cost is unknown, as the
/// controller draws it.
fn chart_numbers(v: &Value, bot: i64, name: &str) -> Value {
    let kept: Value = serde_json::from_str(v["bots"][bot.to_string()]["series"].as_str().unwrap()).unwrap();
    let (value, invested) = (kept["value"].as_array().unwrap(), kept["invested"].as_array().unwrap());
    match name {
        "series" => json!([value, invested]),
        "pnl" if chart_value(v, bot, "received").as_deref() == Some("unknown") => json!([]),
        "pnl" => json!(value.iter().zip(invested).map(|(v, i)| float(v) - float(i)).collect::<Vec<f64>>()),
        other => panic!("no kept series {other}"),
    }
}
fn float(n: &Value) -> f64 { n.as_f64().or_else(|| n.as_str().and_then(|s| s.parse().ok())).unwrap_or_else(|| panic!("not a number: {n}")) }

#[tokio::test]
async fn a_sell_only_chart_is_cumulative_received() {
    // D-SELL-ONLY-CHART: what the sold coins cost is not known, so the chart is what came in, never a profit.
    let install = gemini_seller(false);
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    let chart = shown["bots"]["3"]["chart"].as_str().unwrap();
    let series = chart_numbers(&shown, 3, "series");
    assert_eq!(float(series[0].as_array().unwrap().last().unwrap()), 7.6, "{chart}");
    assert!(series[1].as_array().unwrap().iter().all(|n| float(n) == 0.0), "{chart}");
    assert_eq!(chart_numbers(&shown, 3, "pnl"), json!([]), "{chart}");
    assert_eq!(chart_value(&shown, 3, "received").as_deref(), Some("unknown"), "{chart}");
    assert!(!chart.contains("chart-mode"), "no mode switch: {chart}");
    // A bot that bought is charted as it was.
    assert_eq!(chart_value(&shown, 1, "received"), None, "{}", shown["bots"]["1"]["chart"]);
}

#[tokio::test]
async fn a_sell_only_chart_with_known_cost_draws_the_realised_pnl() {
    let install = gemini_seller(false);
    sold_from_a_purchase(&install);
    walk_and_publish(&install);
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    let chart = shown["bots"]["3"]["chart"].as_str().unwrap();
    let series = chart_numbers(&shown, 3, "series");
    assert_eq!(float(series[0].as_array().unwrap().last().unwrap()), 7.6, "{chart}");
    assert_eq!(float(series[1].as_array().unwrap().last().unwrap()), 6.0, "{chart}");
    // The controller subtracts the two rounded series, as the attribute's P/L was their difference.
    assert!((float(chart_numbers(&shown, 3, "pnl").as_array().unwrap().last().unwrap()) - 1.6).abs() < 1e-9, "{chart}");
    assert_eq!(chart_value(&shown, 3, "received").as_deref(), Some("known"), "{chart}");
    assert!(chart.contains("chart-mode") && chart.contains(">Received<"), "the mode switch, its value mode named Received: {chart}");
}

#[tokio::test]
async fn coins_that_cost_nothing_state_no_pnl() {
    // The walk knows the sold coins cost nothing: no P/L is stated against nothing, in the box, the tile or the chart.
    let install = gemini_seller(false);
    let c = rusqlite::Connection::open(&install.file).unwrap();
    ledger(&c, 1, 2, 0, "0.003", Some(("USDC", "0")), None, "2026-03-01 10:00:00", None, None);
    ledger(&c, 2, 2, 1, "0.002", Some(("USDC", "5")), None, "2026-03-02 14:30:00", Some(3), None);
    ledger(&c, 3, 2, 1, "0.001", Some(("USDC", "2.6")), None, "2026-03-03 14:30:00", Some(4), None);
    let walked = walk_and_publish(&install);
    assert_eq!(walked.bot_sales.iter().map(|s| (s.cost.is_zero(), s.known)).collect::<Vec<_>>(), [(true, true), (true, true)], "{:?}", walked.bot_sales);
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    no_pnl(&shown, 3);
    assert_eq!(chart_value(&shown, 3, "received").as_deref(), Some("unknown"), "{}", shown["bots"]["3"]["chart"]);
}

#[tokio::test]
async fn a_kucoin_bot_is_priced_from_the_venues_tickers_and_counts_in_the_headline() {
    // Exchanges::Kucoin#get_tickers_prices: the whole market from allTickers, through the KuCoin egress. The basket is valued
    // at its last price, and with every bot priced the account has its headline.
    let mut sc = script(true, json!(0.9993));
    sc["GET api.kucoin.com/api/v1/market/allTickers"] = json!({"body": {"code": "200000", "data": {"time": 1, "ticker": [
        {"symbol": "BTC-USDC", "last": "130", "buy": "129.9", "vol": "1"}, {"symbol": "ETH-USDC", "last": "0"}, {"symbol": "XRP-USDC", "last": null}]}}});
    sc["GET data-api:3000/api/v1/prices?coin_ids=usd-coin&vs_currencies=usd"] = json!({"body": {"data": {"usd-coin": {"usd": 1.0}}}});
    let install = install("USD", sc);
    let c = rusqlite::Connection::open(&install.file).unwrap();
    let t = "'2026-01-01'";
    c.execute_batch(&format!("
        INSERT INTO exchanges(id,type,name,created_at,updated_at) VALUES (2,'Exchanges::Kucoin','KuCoin',{t},{t});
        INSERT INTO assets(id,category,external_id,symbol,name,created_at,updated_at) VALUES (5,'Cryptocurrency','usd-coin','USDC','USDC',{t},{t});
        INSERT INTO tickers(id,base,base_asset_id,base_decimals,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,available,trading_enabled,created_at,updated_at)
            VALUES (3,'BTC',4,8,2,0,1,2,'USDC',5,2,'BTC-USDC',1,1,{t},{t});
        INSERT INTO bots(id,user_id,exchange_id,type,status,settings,transient_data,created_at,updated_at) VALUES
            (3,1,2,'Bots::DcaMultiAsset',1,'{{\"quote_asset_id\":5,\"allocations\":{{\"4\":1.0}}}}','{{}}',{t},{t});
        INSERT INTO transactions(bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) VALUES
            (3,2,'BTC',4,'USDC',5,1,1,100,100,0,0,2,'o3','REGULAR','2026-03-02 14:30:00',{t});
    ")).unwrap();
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    let metrics = shown["bots"]["3"]["metrics"].as_str().unwrap();
    assert!(metrics.contains("130.00 <small>USDC</small>") && !metrics.contains("could not be read from the exchange"), "{metrics}");
    assert!(tile(&shown, 3).contains("30.00%"), "{}", tile(&shown, 3));
    assert!(shown["account"].as_str().is_some_and(|h| h.contains("pnl-percent")), "{}", shown["account"]);
}

// ---- S1: figures without an Alpaca key

/// The KuCoin basket of `a_kucoin_bot_is_priced…` on an account with no Alpaca key and no Alpaca bots, its market counted.
fn keyless(script_extra: impl FnOnce(&mut Value)) -> (Install, Arc<std::sync::Mutex<Vec<String>>>) {
    let mut sc = script(true, json!(0.9993));
    sc["GET api.kucoin.com/api/v1/market/allTickers"] = json!({"body": {"code": "200000", "data": {"time": 1, "ticker": [{"symbol": "BTC-USDC", "last": "130"}]}}});
    sc["GET data-api:3000/api/v1/prices?coin_ids=usd-coin&vs_currencies=usd"] = json!({"body": {"data": {"usd-coin": {"usd": 1.0}}}});
    script_extra(&mut sc);
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let install = install_with("USD", Source::Counted(sc, log.clone()));
    let c = rusqlite::Connection::open(&install.file).unwrap();
    let t = "'2026-01-01'";
    c.execute_batch(&format!("
        DELETE FROM api_keys; DELETE FROM transactions; DELETE FROM bots;
        INSERT INTO exchanges(id,type,name,created_at,updated_at) VALUES (2,'Exchanges::Kucoin','KuCoin',{t},{t});
        INSERT INTO assets(id,category,external_id,symbol,name,created_at,updated_at) VALUES (5,'Cryptocurrency','usd-coin','USDC','USDC',{t},{t});
        INSERT INTO tickers(id,base,base_asset_id,base_decimals,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,available,trading_enabled,created_at,updated_at)
            VALUES (3,'BTC',4,8,2,0,1,2,'USDC',5,2,'BTC-USDC',1,1,{t},{t});
        INSERT INTO bots(id,user_id,exchange_id,type,status,settings,transient_data,created_at,updated_at) VALUES
            (3,1,2,'Bots::DcaMultiAsset',1,'{{\"quote_asset_id\":5,\"allocations\":{{\"4\":1.0}}}}','{{}}',{t},{t});
        INSERT INTO transactions(bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) VALUES
            (3,2,'BTC',4,'USDC',5,1,1,100,100,0,0,2,'o3','REGULAR','2026-03-02 14:30:00',{t});
    ")).unwrap();
    (install, log)
}
fn add_key(install: &Install, key: &str) {
    let c = rusqlite::Connection::open(&install.file).unwrap();
    c.execute("INSERT INTO api_keys(user_id,exchange_id,key_type,status,key,secret,passphrase,created_at,updated_at) VALUES (1,1,0,1,?1,?2,?3,'2026-01-01','2026-01-01')",
              rusqlite::params![install.app.cipher.encrypt(key), install.app.cipher.encrypt("secret"), install.app.cipher.encrypt("paper")]).unwrap();
}
fn tickers_asked(log: &std::sync::Mutex<Vec<String>>) -> usize { log.lock().unwrap().iter().filter(|k| k.ends_with("/allTickers")).count() }
async fn ready(install: &Install, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !matches!(loading::prepare(&install.app, 1).await.unwrap(), Snapshot::Ready(..)) {
        assert!(Instant::now() < deadline, "{what}: never ready");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn an_account_without_an_alpaca_key_gets_its_kucoin_figures_and_keeps_them() {
    let (install, log) = keyless(|_| {});
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    let metrics = shown["bots"]["3"]["metrics"].as_str().unwrap();
    assert!(metrics.contains("130.00 <small>USDC</small>") && !metrics.contains("could not be read from the exchange"), "{metrics}");
    assert!(tile(&shown, 3).contains("30.00%"), "{}", tile(&shown, 3));
    assert!(shown["account"].as_str().is_some_and(|h| h.contains("pnl-percent")), "{}", shown["account"]);
    // Warm reads within the price's minute stay ready on the same cache: nothing is asked again.
    for _ in 0..5 {
        assert!(matches!(loading::prepare(&install.app, 1).await.unwrap(), Snapshot::Ready(..)), "a warm read of a keyless account is not ready");
        assert!(install.page().await.is_some());
    }
    assert_eq!(tickers_asked(&log), 1, "{:?}", log.lock().unwrap());
    assert!(log.lock().unwrap().iter().all(|k| !k.contains("data.alpaca.markets")), "no Alpaca request without a key: {:?}", log.lock().unwrap());
}

#[tokio::test]
async fn adding_or_removing_an_alpaca_key_refills_under_the_new_identity() {
    let (install, log) = keyless(|_| {});
    let _stop = install.consume();
    install.shown("keyless").await;
    assert_eq!(tickers_asked(&log), 1);
    // A key added: another producer, so the keyless cache is not reused and the figures are filled again under the key.
    add_key(&install, "key");
    assert!(!matches!(loading::prepare(&install.app, 1).await.unwrap(), Snapshot::Ready(..)), "the keyless figures served for a keyed account");
    ready(&install, "keyed").await;
    assert_eq!(tickers_asked(&log), 2, "{:?}", log.lock().unwrap());
    // Removed again: the keyed figures are not the keyless account's.
    rusqlite::Connection::open(&install.file).unwrap().execute("DELETE FROM api_keys", []).unwrap();
    assert!(!matches!(loading::prepare(&install.app, 1).await.unwrap(), Snapshot::Ready(..)), "the keyed figures served without the key");
    ready(&install, "keyless again").await;
    assert_eq!(tickers_asked(&log), 3, "{:?}", log.lock().unwrap());
}

#[tokio::test]
async fn an_undecryptable_alpaca_key_leaves_its_alpaca_bots_stale_and_the_rest_figured() {
    let mut sc = script(true, json!(0.9993));
    sc["GET api.kucoin.com/api/v1/market/allTickers"] = json!({"body": {"code": "200000", "data": {"time": 1, "ticker": [{"symbol": "BTC-USDC", "last": "130"}]}}});
    sc["GET data-api:3000/api/v1/prices?coin_ids=usd-coin&vs_currencies=usd"] = json!({"body": {"data": {"usd-coin": {"usd": 1.0}}}});
    let install = install("USD", sc);
    let c = rusqlite::Connection::open(&install.file).unwrap();
    // A key this install cannot read (another install's secret, a damaged row): no usable key.
    let mut sealed: Value = serde_json::from_str(&install.app.cipher.encrypt("key")).unwrap();
    sealed["h"]["at"] = json!("AAAAAAAAAAAAAAAAAAAAAA==");
    c.execute("UPDATE api_keys SET key=?1", [sealed.to_string()]).unwrap();
    let t = "'2026-01-01'";
    c.execute_batch(&format!("
        INSERT INTO exchanges(id,type,name,created_at,updated_at) VALUES (2,'Exchanges::Kucoin','KuCoin',{t},{t});
        INSERT INTO assets(id,category,external_id,symbol,name,created_at,updated_at) VALUES (5,'Cryptocurrency','usd-coin','USDC','USDC',{t},{t});
        INSERT INTO tickers(id,base,base_asset_id,base_decimals,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,available,trading_enabled,created_at,updated_at)
            VALUES (3,'BTC',4,8,2,0,1,2,'USDC',5,2,'BTC-USDC',1,1,{t},{t});
        INSERT INTO bots(id,user_id,exchange_id,type,status,settings,transient_data,created_at,updated_at) VALUES
            (3,1,2,'Bots::DcaMultiAsset',1,'{{\"quote_asset_id\":5,\"allocations\":{{\"4\":1.0}}}}','{{}}',{t},{t});
        INSERT INTO transactions(bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) VALUES
            (3,2,'BTC',4,'USDC',5,1,1,100,100,0,0,2,'o3','REGULAR','2026-03-02 14:30:00',{t});
    ")).unwrap();
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    assert!(shown["bots"]["1"]["metrics"].as_str().unwrap().contains("could not be read from the exchange"), "the Alpaca bot is stale: {}", shown["bots"]["1"]["metrics"]);
    assert!(tile(&shown, 3).contains("30.00%"), "the KuCoin bot is priced: {}", tile(&shown, 3));
    // Removing the unreadable key changes the account's identity: the figures are filled again, never served blank.
    c.execute("DELETE FROM api_keys", []).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(v) = install.page().await { if v["bots"]["3"]["tile"].as_str().is_some_and(|t| t.contains("30.00%")) { break; } }
        assert!(Instant::now() < deadline, "the KuCoin figures did not come back after the unreadable key was removed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_key_added_while_a_fill_runs_does_not_make_its_figures_current() {
    // The keyless fill's market answer is held back; the key arrives while it waits.
    let (install, log) = keyless(|sc| { sc["GET api.kucoin.com/api/v1/market/allTickers"]["delay_ms"] = json!(1500); });
    let _stop = install.consume();
    assert!(install.page().await.is_none());
    let deadline = Instant::now() + Duration::from_secs(5);
    while tickers_asked(&log) == 0 { assert!(Instant::now() < deadline, "the fill never asked"); tokio::time::sleep(Duration::from_millis(10)).await; }
    add_key(&install, "key");
    // The keyless fill ends after the key was added: its figures are not kept as the keyed account's. A fill under the key
    // asks again, and only its figures are shown.
    ready(&install, "keyed").await;
    assert_eq!(tickers_asked(&log), 2, "{:?}", log.lock().unwrap());
}

/// A KuCoin basket of BTC bought on 2026-03-02: daily candles (16 days old) from one window, startAt one day before the
/// order and endAt now, as Exchanges::Kucoin#get_candles asks for them. `window` answers that window.
fn kucoin_basket(window: Value) -> Install {
    let mut sc = script(true, json!(0.9993));
    sc["GET api.kucoin.com/api/v1/market/allTickers"] = json!({"body": {"code": "200000", "data": {"time": 1, "ticker": [{"symbol": "BTC-USDC", "last": "130"}]}}});
    sc["GET data-api:3000/api/v1/prices?coin_ids=usd-coin&vs_currencies=usd"] = json!({"body": {"data": {"usd-coin": {"usd": 1.0}}}});
    sc["GET api.kucoin.com/api/v1/market/candles?endAt=1773835410&startAt=1772375400&symbol=BTC-USDC&type=1day"] = window;
    let install = install("USD", sc);
    let c = rusqlite::Connection::open(&install.file).unwrap();
    let t = "'2026-01-01'";
    c.execute_batch(&format!("
        INSERT INTO exchanges(id,type,name,created_at,updated_at) VALUES (2,'Exchanges::Kucoin','KuCoin',{t},{t});
        INSERT INTO assets(id,category,external_id,symbol,name,created_at,updated_at) VALUES (5,'Cryptocurrency','usd-coin','USDC','USDC',{t},{t});
        INSERT INTO tickers(id,base,base_asset_id,base_decimals,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,available,trading_enabled,created_at,updated_at)
            VALUES (3,'BTC',4,8,2,0,1,2,'USDC',5,2,'BTC-USDC',1,1,{t},{t});
        INSERT INTO bots(id,user_id,exchange_id,type,status,settings,transient_data,created_at,updated_at) VALUES
            (3,1,2,'Bots::DcaMultiAsset',1,'{{\"quote_asset_id\":5,\"allocations\":{{\"4\":1.0}}}}','{{}}',{t},{t});
        INSERT INTO transactions(bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) VALUES
            (3,2,'BTC',4,'USDC',5,1,1,100,100,0,0,2,'o3','REGULAR','2026-03-02 14:30:00',{t});
    ")).unwrap();
    install
}

#[tokio::test]
async fn a_kucoin_bots_chart_is_marked_at_the_venues_candles() {
    // Seventeen daily candles, newest first as KuCoin sends them; the one opened today is not closed and is left out.
    let rows: Vec<Value> = (0..17).rev().map(|day| json!([(1_772_409_600 + day * 86_400).to_string(), format!("{}.5", 101 + day), "1", "1", "1", "1", "1"])).collect();
    let install = kucoin_basket(json!({"body": {"code": "200000", "data": rows}}));
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    // The chart's series are its data request's (D-CHART-DATA-REQUEST), kept with the record.
    let chart = shown["bots"]["3"]["series"].as_str().unwrap();
    // Marked at each closed candle's open from the first after the order (03-03, 102.5) to yesterday's (03-17, 116.5), then
    // at the live price.
    assert!(chart.contains("102.5,103.5") && chart.contains("116.5,130.0"), "{chart}");
    assert!(!chart.contains("117.5"), "today's candle is still open: {chart}");
    // The Alpaca bots beside it keep their figures.
    assert!(tile(&shown, 1).contains("10.00%"), "{}", tile(&shown, 1));
}

#[tokio::test]
async fn a_failed_kucoin_window_leaves_that_holding_without_a_grid_and_the_rest_of_the_account_alone() {
    let install = kucoin_basket(json!({"status": 500, "body": {"code": "500000", "msg": "busy"}}));
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    assert!(shown["bots"]["3"]["chart"].as_str().unwrap().contains("hidden"), "no grid: {}", shown["bots"]["3"]["chart"]);
    assert!(tile(&shown, 3).contains("30.00%"), "the bot is still priced: {}", tile(&shown, 3));
    assert!(tile(&shown, 1).contains("10.00%"), "{}", tile(&shown, 1));
}

// ---- Pair bots (Bots::DcaSingleAsset, Bots::Signal)

/// A KuCoin signal bot on BTC-USDT, bought once for 100 tether and priced at 130 by the venue's tickers, beside the dollar
/// basket of AAA on Alpaca (the tether basket removed), the account shown in `currency`.
fn kucoin_pair(currency: &str, tweak: impl FnOnce(&mut Value)) -> Install {
    let mut sc = script(true, json!(0.9993));
    sc["GET api.kucoin.com/api/v1/market/allTickers"] = json!({"body": {"code": "200000", "data": {"time": 1, "ticker": [{"symbol": "BTC-USDT", "last": "130"}]}}});
    tweak(&mut sc);
    let install = install(currency, sc);
    let c = rusqlite::Connection::open(&install.file).unwrap();
    let t = "'2026-01-01'";
    c.execute_batch(&format!("
        DELETE FROM transactions WHERE bot_id=2; DELETE FROM bots WHERE id=2;
        INSERT INTO exchanges(id,type,name,created_at,updated_at) VALUES (2,'Exchanges::Kucoin','KuCoin',{t},{t});
        INSERT INTO tickers(id,base,base_asset_id,base_decimals,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,available,trading_enabled,created_at,updated_at)
            VALUES (3,'BTC',4,8,2,0,1,2,'USDT',3,2,'BTC-USDT',1,1,{t},{t});
        INSERT INTO bots(id,user_id,exchange_id,type,status,settings,transient_data,created_at,updated_at) VALUES
            (3,1,2,'Bots::Signal',1,'{{\"base_asset_id\":4,\"quote_asset_id\":3}}','{{}}',{t},{t});
        INSERT INTO transactions(bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) VALUES
            (3,2,'BTC',4,'USDT',3,1,1,100,100,0,0,2,'o3','REGULAR','2026-03-02 14:30:00',{t});
    ")).unwrap();
    install
}

#[tokio::test]
async fn a_kucoin_pair_beside_an_alpaca_basket_is_converted_through_the_held_tether_rate() {
    let install = kucoin_pair("EUR", |_| {});
    // The pair is priced from KuCoin's tickers: no Alpaca symbol is asked for it.
    assert_eq!(loading::symbols(&rusqlite::Connection::open(&install.file).unwrap(), 1).unwrap(), vec!["AAA".to_string()]);
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    // 30 tether of profit, in dollars at 0.9993, then in euros at 55000/64000.
    assert!(tile(&shown, 3).contains("<small>€</small>25.76") && tile(&shown, 3).contains("30.00%"), "{}", tile(&shown, 3));
    assert!(tile(&shown, 3).starts_with("<div id=\"pnl_bots_signal_3\">"), "{}", tile(&shown, 3));
    assert!(tile(&shown, 1).contains("<small>€</small>8.59"), "{}", tile(&shown, 1));
    assert!(headline(&shown).contains("pnl-percent"), "{}", headline(&shown));
    let metrics = shown["bots"]["3"]["metrics"].as_str().unwrap();
    assert!(metrics.contains("100.00 <small>USDT</small>") && metrics.contains("130.00 <small>USDT</small>"), "{metrics}");
}

#[tokio::test]
async fn a_dropped_tether_rate_empties_only_the_pairs_amount() {
    let install = kucoin_pair("EUR", |sc| { sc["GET data-api:3000/api/v1/prices?coin_ids=tether&vs_currencies=usd"] = json!({"status": 503, "body": {}}); });
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    assert!(tile(&shown, 3).contains("30.00%") && !tile(&shown, 3).contains("pnl-amount"), "{}", tile(&shown, 3));
    assert!(tile(&shown, 1).contains("<small>€</small>8.59"), "{}", tile(&shown, 1));
}

#[tokio::test]
async fn a_raised_pair_price_keeps_the_basket_and_withholds_the_headline() {
    // A signal bot on BTC/USDT at Alpaca: its crypto quote raises, the dollar basket's stock quote answers.
    let mut sc = script(true, json!(0.9993));
    sc["GET data.alpaca.markets/v1beta3/crypto/us/latest/trades"] = json!({"network": "transient", "message": "Faraday::ConnectionFailed: connection refused"});
    let install = install("EUR", sc);
    rusqlite::Connection::open(&install.file).unwrap().execute_batch("UPDATE bots SET type='Bots::Signal', settings='{\"base_asset_id\":4,\"quote_asset_id\":3}' WHERE id=2").unwrap();
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    assert!(tile(&shown, 1).contains("<small>€</small>8.59"), "{}", tile(&shown, 1));
    assert_eq!(shown["bots"]["2"]["tile"], "<div id=\"pnl_bots_signal_2\"></div>");
    assert_eq!(shown["bots"]["2"]["metrics"], "<div id=\"metrics\"></div>");
    assert_eq!(shown["account"], "<div id=\"global-pnl\"></div>", "no total while a traded bot has no figures");
}

#[tokio::test]
async fn a_keyless_account_with_a_kucoin_signal_bot_stays_ready() {
    let (install, log) = keyless(|sc| { sc["GET api.kucoin.com/api/v1/market/allTickers"] = json!({"body": {"code": "200000", "data": {"time": 1, "ticker": [{"symbol": "BTC-USDC", "last": "130"}]}}}); });
    rusqlite::Connection::open(&install.file).unwrap().execute_batch("UPDATE bots SET type='Bots::Signal', settings='{\"base_asset_id\":4,\"quote_asset_id\":5}' WHERE id=3").unwrap();
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    assert!(tile(&shown, 3).contains("30.00%"), "{}", tile(&shown, 3));
    for _ in 0..5 { assert!(matches!(loading::prepare(&install.app, 1).await.unwrap(), Snapshot::Ready(..)), "a warm read of a keyless account is not ready"); }
    assert_eq!(tickers_asked(&log), 1, "{:?}", log.lock().unwrap());
    assert!(log.lock().unwrap().iter().all(|k| !k.contains("data.alpaca.markets")), "{:?}", log.lock().unwrap());
}

#[tokio::test]
async fn a_sell_only_pair_that_received_nothing_shows_a_zero_not_markup() {
    // money() marks a zero with its is-zero span: the Received field keeps that markup, only the currency is escaped.
    let install = kucoin_pair("USD", |_| {});
    let c = rusqlite::Connection::open(&install.file).unwrap();
    c.execute_batch("UPDATE transactions SET side=1, amount_exec=0, quote_amount_exec=0 WHERE bot_id=3").unwrap();
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    let metrics = shown["bots"]["3"]["metrics"].as_str().unwrap();
    assert!(metrics.contains("Received") && metrics.contains("<span class=\"is-zero\">0.00</span> <small>USDT</small>"), "{metrics}");
    assert!(!metrics.contains("&lt;span"), "{metrics}");
}

// ---- PLAN-A17 A17-3: conversions on a CoinGecko install ----

const CG: &str = "GET api.coingecko.com/api/v3";
fn cg_price(coin: &str, currency: &str) -> String {
    format!("{CG}/simple/price?ids={coin}&include_24hr_change=false&include_24hr_vol=false&include_last_updated_at=false&include_market_cap=false&precision=full&vs_currencies={currency}")
}
/// CoinGecko's answers (the key a demo one: the pro ping answers 401); `rates` the exchange_rates reply.
fn cg_script(rates: Value) -> Value {
    let table = json!({"rates": {"btc": {"value": 1}, "usd": {"value": 64000.0}, "eur": {"value": 55000.0}, "gbp": {"value": 48000.0},
                                 "pln": {"value": 236000.0}, "chf": {"value": 52000.0}, "jpy": {"value": 9600000.0}}});
    let rates = if rates.is_null() { json!({"body": table}) } else { rates };
    json!({ "GET pro-api.coingecko.com/api/v3/ping": {"status": 401, "body": {}}, format!("{CG}/exchange_rates"): rates,
            cg_price("tether", "usd"): {"body": {"tether": {"usd": 0.9993}}} })
}
/// The install on the coingecko provider with a key; the log of what it asked.
fn on_coingecko(rates: Value) -> (Install, Arc<std::sync::Mutex<Vec<String>>>) {
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let install = install_with("USD", Source::Counted(cg_script(rates), log.clone()));
    rusqlite::Connection::open(&install.file).unwrap().execute_batch("DELETE FROM app_configs WHERE key LIKE 'market_data_%';
        INSERT INTO app_configs(key,value,created_at,updated_at) VALUES ('market_data_provider','coingecko','2026-01-01','2026-01-01'),('coingecko_api_key','CG-test-key','2026-01-01','2026-01-01');").unwrap();
    (install, log)
}
impl Install {
    async fn fill(&self, pairs: &[(&str, &str)]) {
        let (identity, source) = self.app.db(deltabadger::web::tracker::fx::provider).await.unwrap();
        assert!(source.is_some(), "a CoinGecko key is a provider to ask");
        deltabadger::web::tracker::fx::fill(&self.app, &identity, source, pairs.iter().map(|(f, t)| (f.to_string(), t.to_string())).collect()).await.unwrap();
    }
    fn held(&self, from: &str, to: &str) -> deltabadger::figures::page_market::Held {
        let now = deltabadger::engine::Clock::now(&*self.clock).timestamp();
        self.app.fx_cache.store.get(&self.identity(), from, to, now).unwrap_or_else(|| panic!("{from}->{to} not held"))
    }
}
fn count(log: &Arc<std::sync::Mutex<Vec<String>>>, key: &str) -> usize { log.lock().unwrap().iter().filter(|k| k.as_str() == key).count() }

/// One CoinGecko `exchange_rates` answer serves every fiat pair for 60 s (Coingecko#get_exchange_rates): three pairs in
/// one fill ask it once, a cold pair 30 s later is served by the same answer, one 61 s later asks again. A coin's price
/// is `simple/price` with 2.x's params. Each pair is held as on data-api (12 h fiat and stablecoins).
#[tokio::test]
async fn coingecko_fiat_pairs_share_one_exchange_rates_answer_for_a_minute() {
    let (install, log) = on_coingecko(Value::Null);
    let rates = format!("{CG}/exchange_rates");
    install.fill(&[("USD", "EUR"), ("USD", "GBP"), ("USD", "PLN"), ("USDT", "USD")]).await;
    assert_eq!((count(&log, &rates), count(&log, &cg_price("tether", "usd"))), (1, 1), "{:?}", log.lock().unwrap());
    let now = common::web::at(AT).timestamp();
    let held = install.held("USD", "EUR");
    assert_eq!((held.value, held.until), (Ok(HeldRate::Float(55000.0 / 64000.0)), now + 12 * 3600));
    assert_eq!(install.held("USDT", "USD").value, Ok(HeldRate::Decimal("0.9993".into())));
    install.set("2026-03-18T12:04:00Z");
    install.fill(&[("USD", "CHF")]).await;
    assert_eq!(count(&log, &rates), 1, "the minute's answer serves a cold pair");
    assert_eq!(install.held("USD", "CHF").value, Ok(HeldRate::Float(52000.0 / 64000.0)));
    install.set("2026-03-18T12:04:31Z");
    install.fill(&[("USD", "JPY")]).await;
    assert_eq!(count(&log, &rates), 2, "asked again after 60 s");
    assert_eq!(count(&log, "GET pro-api.coingecko.com/api/v3/ping"), 2, "each fill's client asks the plan once");
}

/// D-CG-FX-FAILURE-HOLD: a failed pair (a 500, or a 429) is held five minutes, not asked again on every read as 2.x
/// asks; a failure is not held at CoinGecko's own level, so a cold pair asks again.
#[tokio::test]
async fn a_failed_coingecko_pair_is_held_five_minutes_and_a_cold_one_asks_again() {
    for status in [500, 429] {
        let (install, log) = on_coingecko(json!({"status": status, "body": {}}));
        let rates = format!("{CG}/exchange_rates");
        install.fill(&[("USD", "EUR")]).await;
        let held = install.held("USD", "EUR");
        assert!(held.value.is_err(), "{status}");
        assert_eq!(held.until, common::web::at(AT).timestamp() + deltabadger::web::tracker::fx::FAILURE_TTL, "{status}");
        install.set("2026-03-18T12:08:29Z");
        install.fill(&[("USD", "EUR")]).await;
        assert_eq!(count(&log, &rates), 1, "{status}: the failed pair is held");
        install.fill(&[("USD", "GBP")]).await;
        assert_eq!(count(&log, &rates), 2, "{status}: a failure is not CoinGecko's held answer");
        install.set("2026-03-18T12:08:30Z");
        install.fill(&[("USD", "EUR")]).await;
        assert_eq!(count(&log, &rates), 3, "{status}: asked again after five minutes");
    }
}

// ---- D-CHART-DATA-REQUEST: GET /bots/{id}/chart/data

const HASH: &str = "$2a$04$abcdefghijklmnopqrstuuKq8n2RkM1bXh0Zc3TtYw5LpJv7dEoGi";
/// A browser signed in as `user` (whose stored password is set to `HASH`, which the session is tied to).
fn browser(install: &Install, user: i64) -> common::web::Browser {
    let c = rusqlite::Connection::open(&install.file).unwrap();
    c.execute("UPDATE users SET encrypted_password=?1 WHERE id=?2", rusqlite::params![HASH, user]).unwrap();
    // The bot page's lookup reads full timestamps; the fixture's are dates.
    c.execute_batch("UPDATE bots SET created_at='2026-01-01 00:00:00', updated_at='2026-01-01 00:00:00'; UPDATE users SET created_at='2026-01-01 00:00:00', updated_at='2026-01-01 00:00:00', confirmed_at='2026-01-01 00:00:00'; UPDATE transactions SET updated_at=created_at").unwrap();
    let data = deltabadger::web::session::SessionData { user: Some((user, HASH[..29].to_string())), ..Default::default() };
    common::web::Browser { cookie: Some(deltabadger::web::session::seal(&install.app.keys.session, &data, install.app.now())), page: None }
}
fn rev_of(shell: &str) -> i64 {
    shell.split("data-bot--chart-rev-value=\"").nth(1).and_then(|rest| rest.split('"').next()).unwrap().parse().unwrap()
}
async fn data(install: &Install, browser: &mut common::web::Browser, path: &str) -> (u16, Value) {
    let answer = browser.send(&install.app, "GET", path, None, common::web::Csrf::None, &[("accept", "application/json")]).await;
    let body = if answer.status == 200 {
        assert_eq!((answer.header("content-type"), answer.header("cache-control")), (Some("application/json"), Some("private, no-store")));
        serde_json::from_str(&answer.body).unwrap()
    } else { Value::String(answer.body.clone()) };
    (answer.status, body)
}

#[tokio::test]
async fn the_chart_shell_carries_no_series_and_its_data_request_answers_them_from_the_same_record() {
    let install = install("USD", script(true, json!(1.0)));
    let mut owner = browser(&install, 1);
    // Cold: no record kept. Nothing is computed in the request; the work is queued.
    assert_eq!(data(&install, &mut owner, "/bots/1/chart/data").await.0, 204);
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    let shell = shown["bots"]["1"]["chart"].as_str().unwrap();
    assert!(shell.contains("data-bot--chart-src-value=\"/bots/1/chart/data\""), "{shell}");
    for series in ["labels", "series", "pnl", "assets", "buys", "prices", "buy-logos"] {
        assert!(!shell.contains(&format!("data-bot--chart-{series}-value")), "{series}: {shell}");
    }
    let (status, answer) = data(&install, &mut owner, "/bots/1/chart/data").await;
    assert_eq!(status, 200);
    assert_eq!(answer["rev"], rev_of(shell), "the revision its shell carries");
    assert_eq!(answer["quote"], "USD");
    let t = answer["t"].as_array().unwrap();
    assert!(t.len() > 2 && t.len() == answer["value"].as_array().unwrap().len() && t.len() == answer["invested"].as_array().unwrap().len(), "{answer}");
    assert_eq!(answer["holdings"], json!(["AAA"]));
    assert!(answer.get("asset").is_none() && answer.get("prices").is_none() && answer.get("buys").is_none(), "the portfolio only: {answer}");
    let (_, asked) = data(&install, &mut owner, "/bots/1/chart/data?asset=AAA&prices=1&buys=1&points=2").await;
    assert_eq!(asked["asset"]["key"], "AAA");
    assert_eq!(asked["asset"]["value"].as_array().unwrap().len(), asked["t"].as_array().unwrap().len());
    assert_eq!(asked["prices"]["AAA"].as_array().unwrap().len(), asked["t"].as_array().unwrap().len());
    assert_eq!(asked["buys"].as_array().unwrap().len(), 1);
    // D-CHART-LIVE: the same query with the answer's `closed` and `sum`, nothing published since: the points from `closed` on.
    let (closed, sum) = (answer["closed"].as_i64().expect("closed"), answer["sum"].as_str().expect("sum"));
    let (status, again) = data(&install, &mut owner, &format!("/bots/1/chart/data?after={closed}&sum={sum}")).await;
    assert_eq!((status, &again["append"]), (200, &json!(true)), "{again}");
    assert!(again["t"].as_array().unwrap().iter().all(|t| t.as_i64().unwrap() >= closed), "{again}");
    let (status, unknown) = data(&install, &mut owner, "/bots/1/chart/data?asset=ZZZ").await;
    assert!(status == 200 && unknown.get("asset").is_none(), "an unknown holding is answered without it");
    for bad in ["points=1", "points=2001", "points=x", "from=x", "from=5&to=4", "prices=yes", "buys=2", "after=5", "sum=ab", "after=x&sum=ab", "after=5&sum="] {
        assert_eq!(data(&install, &mut owner, &format!("/bots/1/chart/data?{bad}")).await.0, 400, "{bad}");
    }
    // Not this account's bot, and signed out.
    rusqlite::Connection::open(&install.file).unwrap().execute("INSERT INTO users(id,email,encrypted_password,setup_completed,confirmed_at,created_at,updated_at) VALUES (2,'u2@example.com','x',1,'2026-01-01','2026-01-01','2026-01-01')", []).unwrap();
    assert_eq!(data(&install, &mut browser(&install, 2), "/bots/1/chart/data").await.0, 404);
    assert_eq!(data(&install, &mut common::web::Browser::default(), "/bots/1/chart/data").await.0, 401);
}

#[tokio::test]
async fn a_stale_record_answers_with_the_rev_its_shell_carries() {
    let install = install("USD", script(true, json!(1.0)));
    let mut owner = browser(&install, 1);
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    let rev = rev_of(shown["bots"]["1"]["chart"].as_str().unwrap());
    // A write moves the revision; the fill it starts cannot run while the figures' one permit is held, so the page reads
    // the stale record meanwhile.
    // The refill's revision is its fill's time: the fill begins with the request below, a minute on.
    install.set("2026-03-18T12:04:30Z");
    let permit = install.app.figure_service.compute().acquire_owned().await.unwrap();
    rusqlite::Connection::open(&install.file).unwrap().execute("INSERT INTO transactions(bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) VALUES (2,1,'BTC',4,'USDT',3,1,1,100,100,0,0,2,'o3','REGULAR','2026-03-03 14:30:00','2026-01-01')", []).unwrap();
    let (status, answer) = data(&install, &mut owner, "/bots/1/chart/data").await;
    assert_eq!((status, &answer["rev"]), (200, &json!(rev)), "{answer}");
    drop(permit);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let (status, answer) = data(&install, &mut owner, "/bots/1/chart/data").await;
        if status == 200 && answer["rev"].as_i64().unwrap() > rev { break; }
        assert!(Instant::now() < deadline, "the refill never answered: {status} {answer}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn a_chart_without_a_candle_grid_is_published_and_its_shell_finds_its_data() {
    // No bars at all: no holding is marked on candles, so 2.x would not cache the marked chart (`chart_cached` false) and
    // the page loads its frame; the publication still carries the shell, and the shell its fill-marked points.
    let mut script = script(true, json!(1.0));
    for key in ["GET data.alpaca.markets/v2/stocks/AAA/bars", "GET data.alpaca.markets/v2/stocks/AAA/bars?adjustment=split", "GET data.alpaca.markets/v1beta3/crypto/us/bars?symbols=BTC/USDT"] {
        script[key] = json!({"body": {"bars": []}});
    }
    let install = install("USD", script);
    let mut owner = browser(&install, 1);
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    assert_eq!((shown["bots"]["1"]["cached"].as_bool(), shown["bots"]["1"]["chart_cached"].as_bool()), (Some(true), Some(false)));
    let streams = install.app.db(move |c| Ok(deltabadger::web::figure::streams(c, 1, &shown).unwrap())).await.unwrap();
    let chart = streams.iter().map(|(_, html)| html).find(|html| html.contains("target=\"chart\"") && html.contains("/bots/1/chart/data")).expect("the chart's shell is published");
    assert!(chart.starts_with("<turbo-stream action=\"replace\" target=\"chart\" method=\"morph\">"), "{chart}");
    let (status, answer) = data(&install, &mut owner, "/bots/1/chart/data").await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["rev"], rev_of(chart));
    // The fill's own mark, then the live price on the last label: no candle between.
    assert_eq!(answer["t"].as_array().unwrap().len(), 2, "{answer}");
    assert_eq!((&answer["value"], &answer["holdings"]), (&json!([100.0, 110.0]), &json!([])), "{answer}");
}
