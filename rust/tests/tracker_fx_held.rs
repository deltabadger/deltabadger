//! The tracker page reads the display currency's rate only as held (`fx::held`): a request never asks the provider.
//! A rate not held, expired, or held for another provider is wanted from the figures queue, which fills it outside any
//! request, for an account with no paper Alpaca key as for any other.
mod common;
use deltabadger::web::{figure::loading::{self, Source}, tracker::fx, App, Config};
use serde_json::json;
use std::{sync::Arc, time::{Duration, Instant}};

const AT: &str = "2026-03-18T12:03:30Z";

/// A tracker-only account (a Binance read-only key, no bots, no paper key) shown in euros, on the hosted provider.
fn install(log: Arc<std::sync::Mutex<Vec<String>>>) -> (tempfile::TempDir, std::path::PathBuf, Arc<common::web::TestClock>, App) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("production.sqlite3");
    let c = rusqlite::Connection::open(&file).unwrap();
    c.execute_batch(include_str!("fixtures/settings_primary_schema.sql")).unwrap();
    let t = "'2026-01-01'";
    c.execute_batch(&format!("
        INSERT INTO exchanges(id,type,name,created_at,updated_at) VALUES (1,'Exchanges::Binance','Binance',{t},{t});
        INSERT INTO app_configs(key,value,created_at,updated_at) VALUES ('market_data_provider','deltabadger',{t},{t}),('market_data_url','http://data-api:3000',{t},{t}),('market_data_token','token',{t},{t});
        INSERT INTO users(id,email,encrypted_password,setup_completed,admin,confirmed_at,display_currency,created_at,updated_at) VALUES (1,'u1@example.com','x',1,1,{t},'EUR',{t},{t});
    ")).unwrap();
    let clock = common::web::TestClock::at(AT);
    let env = common::web::env(common::web::SECRET);
    let script = json!({"GET data-api:3000/api/v1/exchange_rates": {"body": {"data": {"btc": {"value": 1}, "usd": {"value": 64000.0}, "eur": {"value": 55000.0}}}}});
    let app = App::new(Config::from_env(&env).unwrap(), &env, rusqlite::Connection::open(&file).unwrap(), clock.clone()).unwrap()
        .with_figure_source(Source::Counted(script, log)).unwrap();
    c.execute(&format!("INSERT INTO api_keys(user_id,exchange_id,key_type,status,key,secret,created_at,updated_at) VALUES (1,1,2,1,?1,?2,{t},{t})"),
              rusqlite::params![app.cipher.encrypt("key"), app.cipher.encrypt("secret")]).unwrap();
    (dir, file, clock, app)
}

/// The page's rate: the currency's denomination when held, none otherwise.
async fn rate(app: &App) -> Option<String> {
    let page = app.clone();
    app.db(move |c| { let held = fx::held(&page, c, 1)?; Ok(held.denomination(c, 1, page.now().timestamp())?.map(|d| d.rate.to_s_f())) }).await.unwrap()
}

async fn until_held(app: &App, file: &std::path::Path) {
    let identity = fx::provider(&rusqlite::Connection::open(file).unwrap()).unwrap().0;
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.fx_cache.store.get(&identity, "USD", "EUR", app.now().timestamp()).is_none() {
        assert!(Instant::now() < deadline, "the queue filled no rate within 20 s");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn the_page_reads_held_rates_and_the_queue_fills_them_for_a_tracker_only_account() {
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (_dir, file, clock, app) = install(log.clone());
    // Nothing held: the page states none and asks the queue; the provider is not asked.
    assert_eq!(rate(&app).await, None);
    assert!(log.lock().unwrap().is_empty(), "a page asked the provider: {:?}", log.lock().unwrap());
    // The queue fills it (no paper key: the bots' figures never start for this account).
    let (events, rx) = tokio::sync::mpsc::unbounded_channel();
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let follower = tokio::spawn(loading::follow(app.clone(), rx, stopped));
    until_held(&app, &file).await;
    let asked = log.lock().unwrap().len();
    assert_eq!(asked, 1, "one fill: {:?}", log.lock().unwrap());
    // 1 USD = 55000/64000 EUR.
    assert_eq!(rate(&app).await.as_deref(), Some("0.859375"));
    assert_eq!(log.lock().unwrap().len(), asked, "the page read the held rate without asking");
    // Expired: unavailable again until the queue fills it anew.
    clock.set(common::web::at("2026-03-19T00:05:00Z"));
    assert_eq!(rate(&app).await, None);
    until_held(&app, &file).await;
    assert_eq!(rate(&app).await.as_deref(), Some("0.859375"));
    // Another provider: the rate held for the first is not this one's.
    rusqlite::Connection::open(&file).unwrap().execute("UPDATE app_configs SET value='http://data-api-2:3000' WHERE key='market_data_url'", []).unwrap();
    assert_eq!(rate(&app).await, None);
    stop.send(true).unwrap();
    drop(events);
    follower.await.unwrap().unwrap();
}
