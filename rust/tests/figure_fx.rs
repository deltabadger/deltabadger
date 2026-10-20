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
            (1,1,1,'Bots::DcaMultiAsset',1,'{{\"quote_asset_id\":1,\"allocations\":{{\"2\":1.0}}}}','{{}}',{t},{t}),
            (2,1,1,'Bots::DcaMultiAsset',1,'{{\"quote_asset_id\":3,\"allocations\":{{\"4\":1.0}}}}','{{}}',{t},{t});
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

/// A listed divergence, not parity: the provider sent tether's price as a String. Rails' User#global_pnl multiplies by
/// it and raises (`String can't be coerced`), a Rails defect on a display-only figure; the server reads the decimal it
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
    // The crypto quote raises (a transient network failure): the tether basket's figures stay loading, as Rails' metrics job for
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

#[tokio::test]
async fn a_sell_only_bot_on_a_venue_the_figures_do_not_read_says_what_it_sold_and_received() {
    // 3.0, Rust only: a bot that only sells (what it sells came from outside) states what it sold and what that brought
    // in, not an investment. Its venue is not read yet, which is no failure of the venue's: no note that it did not answer.
    let install = install("USD", script(true, json!(1.0)));
    let c = rusqlite::Connection::open(&install.file).unwrap();
    let t = "'2026-01-01'";
    c.execute_batch(&format!("
        INSERT INTO exchanges(id,type,name,created_at,updated_at) VALUES (2,'Exchanges::Bitget','Bitget',{t},{t});
        INSERT INTO assets(id,category,external_id,symbol,name,created_at,updated_at) VALUES (5,'Cryptocurrency','usd-coin','USDC','USDC',{t},{t});
        INSERT INTO tickers(id,base,base_asset_id,base_decimals,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,available,trading_enabled,created_at,updated_at)
            VALUES (3,'BTC',4,8,2,0,1,2,'USDC',5,2,'BTCUSDC',1,1,{t},{t});
        INSERT INTO bots(id,user_id,exchange_id,type,status,settings,transient_data,created_at,updated_at) VALUES
            (3,1,2,'Bots::DcaMultiAsset',1,'{{\"quote_asset_id\":5,\"allocations\":{{\"4\":1.0}},\"direction\":\"selling\"}}','{{}}',{t},{t});
        INSERT INTO transactions(bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at) VALUES
            (3,2,'BTC',4,'USDC',5,0.002,0.002,2500,5,1,0,2,'o3','REGULAR','2026-03-02 14:30:00',{t}),
            (3,2,'BTC',4,'USDC',5,0.001,0.001,2600,2.6,1,0,2,'o4','REGULAR','2026-03-03 14:30:00',{t});
    ")).unwrap();
    let _stop = install.consume();
    let shown = install.shown("ready").await;
    let metrics = shown["bots"]["3"]["metrics"].as_str().unwrap();
    assert!(metrics.contains(">Sold</div>") && metrics.contains("0.003 <small>BTC</small>"), "{metrics}");
    assert!(metrics.contains(">Received</div>") && metrics.contains("7.60 <small>USDC</small>"), "{metrics}");
    assert!(!metrics.contains("Total invested") && !metrics.contains("does not respond"), "{metrics}");
    // A bot that bought keeps its investment.
    assert!(shown["bots"]["1"]["metrics"].as_str().unwrap().contains("Total invested"));
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
    assert!(metrics.contains("130.00 <small>USDC</small>") && !metrics.contains("does not respond"), "{metrics}");
    assert!(tile(&shown, 3).contains("30.00%"), "{}", tile(&shown, 3));
    assert!(shown["account"].as_str().is_some_and(|h| h.contains("pnl-percent")), "{}", shown["account"]);
}
