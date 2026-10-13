use super::common;
use deltabadger::engine::events::EngineEvent;
use deltabadger::figures::{at::At, page_market::{Cache, Reader}};

/// A clock that, once `armed` with a count of readings to let pass, reports the next one (`read`) and holds it until
/// told (`go`). A publication reads the clock in `begin`, right after it read the account, and once more as it records
/// what it rendered against that read (`Service::record`), just before it delivers: holding that second reading lets a
/// test commit a write after every check the publication makes, deterministically.
/// (readings still to let pass, where to report the held one, what releases it)
type Armed = Option<(usize, std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>;
struct Gate { clock: std::sync::Arc<common::web::TestClock>, armed: std::sync::Mutex<Armed> }
impl deltabadger::engine::Clock for Gate {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        let mut armed = self.armed.lock().unwrap();
        let held = match armed.as_mut() { Some((0, ..)) => armed.take(), Some((pass, ..)) => { *pass -= 1; None } None => None };
        drop(armed);
        if let Some((_, read, go)) = held { let _ = read.send(()); let _ = go.recv(); }
        deltabadger::engine::Clock::now(&*self.clock)
    }
}

/// Alpaca over a scripted transport, for the engine on a copy of a scenario's database.
struct Scripted(deltabadger::venue::http::ScriptedTransport);
impl deltabadger::venue::VenueFactory for Scripted {
    type V = deltabadger::venue::alpaca::AlpacaVenue<deltabadger::venue::http::ScriptedTransport>;
    fn for_bot(&self, _: &str, _: Option<deltabadger::crypto::Credentials>) -> Self::V {
        deltabadger::venue::alpaca::AlpacaVenue::new(self.0.clone(), deltabadger::venue::alpaca::Urls::for_passphrase(Some("paper")))
    }
}
use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::time::Duration;

type Socket = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A page's `/cable` connection, as turbo-rails opens it: the page's streams subscribed, the app served on a port of its own.
/// `early`: what arrived before every subscription was confirmed, which `next` gives first.
struct Cable { socket: Socket, streams: HashMap<String, String>, server: tokio::task::JoinHandle<()>, frames: usize, early: std::collections::VecDeque<Value> }
impl Drop for Cable { fn drop(&mut self) { self.server.abort(); } }
impl Cable {
    async fn open(app: &deltabadger::web::App, cookie: &str, streams: &BTreeSet<String>) -> Self {
        use futures_util::SinkExt;
        use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let served = app.clone();
        let server = tokio::spawn(async move { let _ = deltabadger::web::server::serve_on(listener, served, Default::default()).await; });
        let mut request = format!("ws://{address}/cable").into_client_request().unwrap();
        request.headers_mut().insert("origin", format!("http://{address}").parse().unwrap());
        request.headers_mut().insert("sec-websocket-protocol", "actioncable-v1-json".parse().unwrap());
        request.headers_mut().insert("cookie", format!("_deltabadger_rust_session={cookie}").parse().unwrap());
        let socket = tokio::time::timeout(Duration::from_secs(5), tokio_tungstenite::connect_async(request)).await.unwrap().unwrap().0;
        let mut cable = Cable { socket, streams: HashMap::new(), server, frames: 0, early: Default::default() };
        for stream in streams {
            let identifier = json!({"channel": "Turbo::StreamsChannel", "signed_stream_name": deltabadger::web::cable::signed_stream_name(&app.keys.streams, stream)}).to_string();
            cable.socket.send(Message::Text(json!({"command": "subscribe", "identifier": identifier}).to_string().into())).await.unwrap();
            cable.streams.insert(identifier, stream.clone());
        }
        let (mut confirmed, mut early) = (0, std::collections::VecDeque::new());
        while confirmed < streams.len() {
            let value = cable.next().await;
            if value["type"] == "confirm_subscription" { confirmed += 1; } else { early.push_back(value); }
        }
        cable.early = early;
        cable
    }

    async fn next(&mut self) -> Value {
        use futures_util::StreamExt;
        if let Some(value) = self.early.pop_front() { return value; }
        let message = self.socket.next().await.expect("the server closed /cable").unwrap();
        serde_json::from_str(message.to_text().unwrap()).unwrap()
    }

    /// What arrives, by (stream, target), until each of `wanted` has: the last payload of each.
    async fn until(&mut self, wanted: &BTreeSet<(String, String)>) -> BTreeMap<(String, String), String> {
        let mut got = BTreeMap::new();
        let _ = tokio::time::timeout(Duration::from_secs(10), async {
            while !wanted.iter().all(|key| got.contains_key(key)) {
                let value = self.next().await;
                let (Some(identifier), Some(html)) = (value["identifier"].as_str(), value["message"].as_str()) else { continue };
                got.insert((self.streams[identifier].clone(), target(html)), html.to_string());
                self.frames += 1;
            }
        }).await;
        got
    }

    /// Everything already sent, counted in `frames`: a marker on `stream` comes after it.
    async fn drain(&mut self, app: &deltabadger::web::App, stream: &str) {
        app.hub.broadcast(stream, "drained");
        loop {
            let value = self.next().await;
            if value["message"] == "drained" { return; }
            if value["message"].is_string() { self.frames += 1; }
        }
    }
}

fn target(html: &str) -> String {
    let document = scraper::Html::parse_fragment(html);
    let selector = scraper::Selector::parse("turbo-stream").unwrap();
    document.select(&selector).next().and_then(|stream| stream.value().attr("target")).unwrap_or_default().to_string()
}

/// Each payload `/cable` delivered is the one Rails broadcast to that stream and target.
fn delivered(name: &str, when: &str, got: &BTreeMap<(String, String), String>, rails: &BTreeMap<(String, String), String>) {
    let missing: Vec<_> = rails.keys().filter(|key| !got.contains_key(*key)).collect();
    assert!(missing.is_empty(), "{name}: {when}: these broadcasts never arrived: {missing:?}");
    for (key, payload) in rails {
        assert_eq!(common::html::normalize(&got[key]), common::html::normalize(payload), "{name}: {when}: {key:?}");
    }
}

/// The bot page's chart frame: (its `src`, the frame itself).
fn bot_chart(body: &str) -> (Option<String>, Vec<String>) {
    let doc = scraper::Html::parse_document(body);
    let frame = doc.select(&scraper::Selector::parse("turbo-frame#bot_chart").unwrap()).next().expect("no bot_chart frame");
    (frame.value().attr("src").map(str::to_string), common::html::normalize(&frame.html()))
}

/// A browser on the bot page, then on its chart frame at the `src` the page gave it, against what Rails answered in the
/// same `state` (pages.rb figures: page_figures.rb#pages). Returns the frame's body.
async fn chart_frames(name: &str, state: &str, id: &str, browser: &mut common::web::Browser, app: &deltabadger::web::App, rails: &Value) -> String {
    let (page, rails_page) = (browser.get(app, &format!("/bots/{id}")).await, bot_chart(rails["page"].as_str().unwrap()));
    assert_eq!(page.status, 200, "{name} {state}: {}", page.body);
    let ours = bot_chart(&page.body);
    assert_eq!(ours, rails_page, "{name} {state}: the page's chart frame");
    let chart = browser.get(app, ours.0.as_deref().unwrap()).await;
    assert_eq!(chart.status, 200, "{name} {state}");
    assert_eq!(bot_chart(&chart.body).1, bot_chart(rails["chart"].as_str().unwrap()).1, "{name} {state}: the chart frame");
    chart.body
}

/// What the dashboard asks for once its streams are connected: the attributes of its headline and of its tiles.
async fn dashboard(name: &str, state: &str, browser: &mut common::web::Browser, app: &deltabadger::web::App, rails: &Value) {
    let Some(rails) = rails.as_str() else { return }; // one bot: Rails sends its dashboard to the bot's page
    let asks = |body: &str| -> Vec<Vec<(String, String)>> {
        let doc = scraper::Html::parse_document(body);
        ["section.dash-intro", "div.itiles"].iter().map(|css| {
            let element = doc.select(&scraper::Selector::parse(css).unwrap()).next().unwrap_or_else(|| panic!("{name} {state}: no {css}"));
            let mut attrs: Vec<(String, String)> = element.value().attrs().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            attrs.sort();
            attrs
        }).collect()
    };
    let page = browser.get(app, "/bots").await;
    assert_eq!(page.status, 200, "{name} {state}: {}", page.body);
    assert_eq!(asks(&page.body), asks(rails), "{name} {state}: the dashboard's asks");
}

/// A connection that subscribes again is sent the account's last publication only while nothing the figures read has
/// changed (`loading::resubscribed`): a ticker disabled, a split's ratio corrected, a ledger row resolved to another
/// asset, a listing on a venue the bot left corrected, an asset only an order or a bot's settings name changed, a
/// composition ticker on another venue resized, or an operation flag set or cleared refuse the replay; a sync that
/// rewrites a row's timestamps (and the reference data the figures never read) and other transient bookkeeping keep it. On a copy of the scenario's install, so that the rest of the scenario reads the one Rails recorded.
async fn replays_follow_what_the_figures_read(env: &impl Fn(&str) -> Option<String>, dir: &Path, sc: &Value, user: i64, stream: &str) {
    use deltabadger::web::{App,Config,figure::loading::{self,Source}};
    let at = sc["at"].as_str().unwrap();
    let ccc: i64 = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap()
        .query_row("SELECT base_asset_id FROM tickers WHERE ticker='CCC'", [], |r| r.get(0)).unwrap();
    // A venue and a listing on it, copied from AAA's, for the cases below that read another venue.
    let venue = |id: i64| format!("INSERT INTO exchanges(id,name,type,created_at,updated_at) VALUES ({id},'Old {id}','Exchanges::Kraken','2020-01-01','2020-01-01');");
    let listing = |id: i64, exchange: i64, base: &str, asset: i64| format!("INSERT INTO tickers(id,base,base_asset_id,base_decimals,created_at,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,updated_at) \
        SELECT {id},'{base}',{asset},base_decimals,created_at,{exchange},minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,'{base}/USD',updated_at FROM tickers WHERE ticker='AAA';");
    let asset = |id: i64, symbol: &str| format!("INSERT INTO assets(id,category,created_at,external_id,name,symbol,updated_at) VALUES ({id},'Stock','2020-01-01','{symbol}','{symbol} Inc','{symbol}','2020-01-01');");
    let aaa: i64 = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap()
        .query_row("SELECT base_asset_id FROM tickers WHERE ticker='AAA'", [], |r| r.get(0)).unwrap();
    let alpaca: i64 = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap()
        .query_row("SELECT exchange_id FROM tickers WHERE ticker='AAA'", [], |r| r.get(0)).unwrap();
    // (what, the state the figures are published in, the edit after, whether the publication is still replayed)
    let mut cases: Vec<(String, String, String, bool)> = [
        ("a sync that rewrites timestamps and reference data the figures never read", String::new(),
         "UPDATE tickers SET updated_at='2030-01-01 00:00:00', ath=1234.5, ath_updated_at='2030-01-01 00:00:00'; UPDATE assets SET updated_at='2030-01-01 00:00:00', market_cap=coalesce(market_cap,0)+1, market_cap_rank=7; \
          UPDATE account_transactions SET updated_at='2030-01-01 00:00:00'".to_string(), true),
        ("a held ticker disabled", String::new(), "UPDATE tickers SET available=0 WHERE ticker='AAA'".to_string(), false),
        ("a held asset recategorised", String::new(), "UPDATE assets SET category='Cryptocurrency' WHERE id=(SELECT base_asset_id FROM tickers WHERE ticker='BBB')".to_string(), false),
        ("a split's ratio corrected in place", String::new(), "UPDATE account_transactions SET raw_data='{\"corporate_action\":\"split\",\"split_ratio\":\"2:1\"}' WHERE entry_type=15".to_string(), false),
        ("a ledger row resolved to another asset", String::new(), format!("UPDATE account_transactions SET base_asset_id={ccc} WHERE entry_type=15"), false),
        // The bot traded AAA on a venue it left, as OLD there; the split reported there names no asset, so its venue's
        // listing decides which holding it is.
        ("the asset a left venue lists a split's name under corrected",
         format!("{}{}UPDATE transactions SET exchange_id=2, base='OLD' WHERE id=(SELECT min(id) FROM transactions WHERE base='AAA'); \
                  INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_asset_id,base_amount,raw_data,transacted_at,created_at,updated_at) \
                  VALUES ({user},2,15,'OLD',NULL,0,'{{\"corporate_action\":\"split\",\"split_ratio\":\"3:1\"}}','2021-01-01 00:00:00','{at}','{at}');", venue(2), listing(100,2,"OLD",ccc)),
         format!("UPDATE tickers SET base_asset_id={aaa} WHERE id=100"), false),
        ("other transient bookkeeping", String::new(), "UPDATE bots SET transient_data=json_set(transient_data,'$.missed_quote_amount',5)".to_string(), true),
        // Read by no listing: an asset only an order names, and one only a bot's settings name.
        ("an asset only an order names renamed",
         format!("{}UPDATE transactions SET base='ZZZ', base_asset_id=20 WHERE id=(SELECT min(id) FROM transactions WHERE base='BBB');", asset(20,"ZZZ")),
         "UPDATE assets SET name='Zed Holdings' WHERE id=20".to_string(), false),
        ("an asset only a bot's settings name removed",
         format!("{}UPDATE bots SET settings=json_set(settings,'$.allocations',json('{{\"21\":1.0}}')) WHERE id=(SELECT bot_id FROM bot_index_assets GROUP BY bot_id HAVING count(*)=1);", asset(21,"YYY")),
         "DELETE FROM assets WHERE id=21".to_string(), false),
        // A composition member's ticker is read by its id, whichever venue lists it.
        ("a composition ticker on another venue resized",
         format!("{}{}UPDATE bot_index_assets SET ticker_id=101 WHERE bot_id=(SELECT bot_id FROM bot_index_assets GROUP BY bot_id HAVING count(*)=1);", venue(3), listing(101,3,"AAA",aaa)),
         "UPDATE tickers SET minimum_quote_size=7 WHERE id=101".to_string(), false),
        // A listing of the bots' venue that no bot trades, no order names and no split's name resolves to; delisted, so
        // that no bot prices it.
        ("an unrelated listing on the bots' venue resized",
         format!("{}{}UPDATE tickers SET available=0 WHERE id=102;", asset(30,"UNR"), listing(102,alpaca,"UNR",30)),
         "UPDATE tickers SET minimum_quote_size=7, minimum_base_size=3 WHERE id=102".to_string(), true),
        // The index reads every tradable listing of its venue in its quote: the coarsest quote precision among them rounds
        // its chart (chart::quote_decimals). One the account never held, re-precisioned, is read; one on another venue is not.
        ("a never-held tradable listing in the index's venue and quote re-precisioned",
         format!("{}{}", asset(31,"NVR"), listing(103,alpaca,"NVR",31)),
         "UPDATE tickers SET quote_decimals=0 WHERE id=103".to_string(), false),
        ("a never-held tradable listing in the index's venue and quote renamed",
         format!("{}{}", asset(33,"NVS"), listing(105,alpaca,"NVS",33)),
         "UPDATE tickers SET ticker='NVS2' WHERE id=105".to_string(), false),
        ("a listing on a venue the account never used re-precisioned",
         format!("{}{}{}", venue(4), asset(32,"OTH"), listing(104,4,"OTH",32)),
         "UPDATE tickers SET quote_decimals=0 WHERE id=104".to_string(), true),
    ].into_iter().map(|(what, prep, sql, kept)| (what.to_string(), prep, sql, kept)).collect();
    // What hides the Sell and Redeploy controls while an operation runs.
    for flag in ["rebalance_pending", "liquidation_pending", "liquidation_selling_since", "redeploy_pending"] {
        let set = format!("UPDATE bots SET transient_data=json_set(transient_data,'$.{flag}','2026-01-01T00:00:00Z')");
        cases.push((format!("{flag} set"), String::new(), set.clone(), false));
        cases.push((format!("{flag} cleared"), set, format!("UPDATE bots SET transient_data=json_remove(transient_data,'$.{flag}')"), false));
    }
    let mut wrong = vec![];
    // Each edit on a fresh copy, so that one edit's refusal leaves the next its own warm publication.
    for (what, prep, sql, kept) in cases {
        let copy = tempfile::tempdir().unwrap();
        let file = copy.path().join("production.sqlite3");
        std::fs::copy(dir.join("production.sqlite3"), &file).unwrap();
        let c = rusqlite::Connection::open(&file).unwrap();
        // A split already in the ledger, for the edits below to correct.
        c.execute("INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_asset_id,base_amount,raw_data,transacted_at,created_at,updated_at) \
                   SELECT ?1,exchange_id,15,'AAA',base_asset_id,0,'{\"corporate_action\":\"split\",\"split_ratio\":\"10:1\"}','2020-01-01 00:00:00',?2,?2 FROM tickers WHERE ticker='AAA'", rusqlite::params![user, at]).unwrap();
        c.execute_batch(&prep).unwrap();
        let app = App::new(Config::from_env(env).unwrap(), env, rusqlite::Connection::open(&file).unwrap(), common::web::TestClock::at(at)).unwrap()
            .with_figure_source(Source::Script(sc["script"].clone())).unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while !matches!(loading::prepare(&app, user).await.unwrap(), loading::Snapshot::Ready(..)) { tokio::time::sleep(Duration::from_millis(20)).await; }
        }).await.expect("the figures never became ready");
        loading::publish(&app, user).await.unwrap();
        assert!(!loading::resubscribed(&app, user, stream).await.is_empty(), "a fresh publication is replayed");
        c.execute_batch(&sql).unwrap();
        if loading::resubscribed(&app, user, stream).await.is_empty() == kept { wrong.push(format!("{what}: replayed {}", if kept { "nothing" } else { "the old figures" })); }
    }
    // The fingerprint reads the account's rows, not its venues' whole listings: 10,000 more listings on the bots' venue,
    // tradable in the bots' quote, of assets nothing in the account names, add no row to it.
    let copy = tempfile::tempdir().unwrap();
    let file = copy.path().join("production.sqlite3");
    std::fs::copy(dir.join("production.sqlite3"), &file).unwrap();
    let c = rusqlite::Connection::open(&file).unwrap();
    c.execute("INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_asset_id,base_amount,raw_data,transacted_at,created_at,updated_at) \
               SELECT ?1,exchange_id,15,'AAA',base_asset_id,0,'{\"corporate_action\":\"split\",\"split_ratio\":\"10:1\"}','2020-01-01 00:00:00',?2,?2 FROM tickers WHERE ticker='AAA'", rusqlite::params![user, at]).unwrap();
    let read = |c: &rusqlite::Connection| { let started = std::time::Instant::now(); let (_, rows) = loading::revision(c, user).unwrap(); (rows, started.elapsed()) };
    let (alone, _) = read(&c);
    c.execute_batch("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<10000) \
        INSERT INTO assets(id,category,created_at,external_id,name,symbol,updated_at) SELECT 200000+i,'Stock','2020-01-01','U'||i,'U'||i||' Inc','U'||i,'2020-01-01' FROM n; \
        WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<10000) \
        INSERT INTO tickers(id,base,base_asset_id,base_decimals,created_at,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,updated_at) \
        SELECT 200000+i,'U'||i,200000+i,t.base_decimals,t.created_at,t.exchange_id,t.minimum_base_size,t.minimum_quote_size,t.price_decimals,t.quote,t.quote_asset_id,t.quote_decimals,'U'||i||'/USD',t.updated_at \
        FROM n, (SELECT * FROM tickers WHERE ticker='AAA') t;").unwrap();
    let (crowded, took) = read(&c);
    eprintln!("revision beside 10,000 unrelated listings: {took:?}, {crowded} rows read ({alone} without them)");
    if crowded != alone { wrong.push(format!("the fingerprint read {} rows beside 10,000 unrelated listings, {alone} without them", crowded)); }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

struct Wire { script: Value, calls: RefCell<Vec<String>> }
impl Transport for Wire {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        let picked: Vec<_> = r.query.iter().filter(|(k,_)| r.path.ends_with("/bars") && (*k == "adjustment" || *k == "symbols")).collect();
        let suffix = if picked.is_empty() { String::new() } else { format!("?{}", picked.iter().map(|(k,v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")) };
        let key = format!("GET data.alpaca.markets{}{suffix}", r.path);
        self.calls.borrow_mut().push(key.clone());
        let reply = self.script.get(&key).unwrap_or_else(|| panic!("unscripted {key}"));
        Ok(HttpResponse { status: reply["status"].as_u64().unwrap_or(200) as u16, body: reply["body"].as_str().map(str::to_string).unwrap_or_else(|| reply["body"].to_string()) })
    }
}
/// `script/rust/pages.rb <command> <scratch>`, as pages.rs runs Rails: the test environment, booted against scratch
/// databases only. Development would check the checkout's own databases for pending migrations on every page and answer
/// 500 where they are missing (CI).
fn record(scratch: &tempfile::TempDir, command: &str) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let boot = tempfile::tempdir().unwrap();
    let mut cmd = std::process::Command::new(root.join("bin/rails"));
    cmd.current_dir(root).args(["runner", "script/rust/pages.rb", command, scratch.path().to_str().unwrap()]).env_remove("DATABASE_URL")
        .env("RAILS_ENV", "test").env("SECRET_KEY_BASE",common::web::SECRET).env("APP_ROOT_URL", "http://localhost:3000").env("SKIP_TEST_DATABASE", "true");
    for db in ["primary", "queue", "cache", "cable"] {
        cmd.env(format!("{}_DATABASE_URL", db.to_uppercase()), format!("sqlite3:{}/{db}.sqlite3", boot.path().display()));
    }
    let out = cmd.output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}
#[tokio::test]
async fn figures_fragments_match_the_rails_page_partials() {
    let scratch = tempfile::tempdir().unwrap();
    record(&scratch, "figures");
    let mut currencies = 0;
    for entry in std::fs::read_dir(scratch.path()).unwrap() {
        let dir = entry.unwrap().path();
        let name = dir.file_name().unwrap().to_str().unwrap();
        if name == "boundaries" { boundaries(&dir); currencies += 100; continue; }
        let sc: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("scenario.json")).unwrap()).unwrap();
        let expected: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("rails.json")).unwrap()).unwrap();
        if name.starts_with("fx_") { display_currency(name, &dir, &sc, &expected).await; currencies += 1; continue; }
        let c = rusqlite::Connection::open_with_flags(dir.join("production.sqlite3"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let now = At::from_utc(chrono::DateTime::parse_from_rfc3339(sc["at"].as_str().unwrap()).unwrap().to_utc()).unwrap();
        let wire = Wire { script: sc["script"].clone(), calls: RefCell::default() };
        let mut cache = Cache::default();
        for _ in 0..4 {
            let reader = Reader::new(&cache, now.utc().timestamp());
            let _ = deltabadger::web::figure::account(&c, sc["user_id"].as_i64().unwrap(), &reader, now, "en", "token", "");
            let demands = reader.demands();
            if demands.is_empty() { break; }
            cache.fill(&wire, demands, now.utc().timestamp()).await;
        }
        if ["row_readings","tax_lots"].contains(&name) {
            // RULING-R2: Rails publishes a number after skipping an executed, unpriced fill.
            let reader=Reader::new(&cache,now.utc().timestamp());
            let err=deltabadger::web::figure::account(&c,sc["user_id"].as_i64().unwrap(),&reader,now,"en","token","").unwrap_err();
            assert!(matches!(err,deltabadger::figures::FiguresError::NotComputed(ref why) if why=="executed fill value unavailable"));
            assert!(!expected["bots"]["1"]["tile"].as_str().unwrap().contains("no-value"),"Rails recorded its skipped-row figure");
            use deltabadger::web::{App,Config,figure::loading::{self,Snapshot,Source}};
            // The fill fails, and keeps why with the failure: the page reads no order to say it.
            let env=common::web::env(common::web::SECRET);
            let app=App::new(Config::from_env(&env).unwrap(),&env,rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap(),common::web::TestClock::at(sc["at"].as_str().unwrap())).unwrap().with_figure_source(Source::Script(sc["script"].clone())).unwrap();
            let user=sc["user_id"].as_i64().unwrap();
            let failed=tokio::time::timeout(std::time::Duration::from_secs(10),async { loop {
                match loading::prepare(&app,user).await.unwrap() { Snapshot::Cold=>tokio::time::sleep(std::time::Duration::from_millis(20)).await, other=>break other }
            }}).await.unwrap();
            assert!(matches!(failed,Snapshot::Failed(Some("executed fill value unavailable"))),"{name}: the fill keeps its reason");
            let rendered=loading::render(&c,user,&failed,"en","token","",&*app.clock).unwrap().unwrap();
            let absent="<span class=\"no-value\">Figures unavailable: executed fill value unavailable</span>";
            assert_eq!(rendered,serde_json::json!({"bots":{"1":{
                "tile":format!("<div id=\"pnl_bots_dca_multi_asset_1\">{absent}</div>"),
                "metrics":format!("<div id=\"metrics\">{absent}</div>"),
                "chart":format!("<div id=\"chart\">{absent}</div>")}},
                "account":format!("<div id=\"global-pnl\">{absent}</div>")}));
            let streams=deltabadger::web::figure::streams(&c,sc["user_id"].as_i64().unwrap(),&rendered).unwrap();
            assert_eq!(streams.len(),4);
            for (_,payload) in streams {assert!(payload.contains(absent));assert!(!payload.contains("data-bot--chart-series"));}
            continue; // Exact unavailable output above replaces the named Rails money defect.
        }
        if std::env::var("FIGURE_ROUTES").as_deref() != Ok("0") && ["basket_buys", "hidden", "owner_account"].contains(&name) {
            use deltabadger::web::{App,Config,session::{self,SessionData},figure::loading::{self,Source}};
            let env=common::web::env(common::web::SECRET);
            let clock=common::web::TestClock::at(sc["at"].as_str().unwrap());
            // Every stock snapshot answers after 300 ms, so that a fill is still in flight when the page asks.
            let mut held=sc["script"].clone();
            for (key,reply) in held.as_object_mut().unwrap() { if key.contains("/snapshots") { reply["delay_ms"]=json!(300); } }
            let app=App::new(Config::from_env(&env).unwrap(),&env,rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap(),clock).unwrap().with_figure_source(Source::Script(held.clone())).unwrap();
            let user=sc["user_id"].as_i64().unwrap();
            let password:String=c.query_row("SELECT encrypted_password FROM users WHERE id=?1",[user],|r|r.get(0)).unwrap();
            let signed=SessionData { user:Some((user,password.chars().take(29).collect())),..SessionData::default() };
            let cookie=session::seal(&app.keys.session,&signed,now.utc());
            let mut browser=common::web::Browser { cookie:Some(cookie.clone()),page:None };
            // Cold: before any figure is known, the page sends its chart frame for them, and the frame asks.
            let cold=App::new(Config::from_env(&env).unwrap(),&env,rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap(),common::web::TestClock::at(sc["at"].as_str().unwrap())).unwrap().with_figure_source(Source::Script(held.clone())).unwrap();
            for id in sc["bot_ids"].as_array().unwrap().iter().map(|id|id.to_string()) { chart_frames(name,"cold",&id,&mut browser,&cold,&expected["cold"]["bots"][&id]).await; }
            dashboard(name,"cold",&mut browser,&cold,&expected["cold"]["dashboard"]).await;
            drop(cold);
            // What Rails broadcast for every figure of the account, by (stream, target), and the streams a page subscribes.
            let rails:BTreeMap<(String,String),String>=expected["broadcasts"].as_array().unwrap().iter()
                .map(|entry|((entry[0].as_str().unwrap().to_string(),target(entry[1].as_str().unwrap())),entry[1].as_str().unwrap().to_string())).collect();
            let wanted:BTreeSet<(String,String)>=rails.keys().cloned().collect();
            let streams:BTreeSet<String>=wanted.iter().map(|(stream,_)|stream.clone()).collect();
            let user_stream=format!("user_{user}:bot_updates");
            if name == "owner_account" { replays_follow_what_the_figures_read(&env,&dir,&sc,user,&user_stream).await; }
            // Cold to warm: the page's streams are connected before any figure is known, the page asks while the fill is in
            // flight (its first market answer is held back), and the end of the fill replaces every spinner.
            let mut cable=Cable::open(&app,&cookie,&streams).await;
            assert!(matches!(loading::prepare(&app,user).await.unwrap(),loading::Snapshot::Cold));
            loading::publish(&app,user).await.unwrap();
            delivered(name,"the end of the fill",&cable.until(&wanted).await,&rails);
            tokio::time::timeout(std::time::Duration::from_secs(5),async {
                loop {
                    match loading::prepare(&app,user).await.unwrap() {
                        loading::Snapshot::Ready(..)=>break,
                        loading::Snapshot::Failed(_)=>panic!("fill failed"),
                        _=>tokio::task::yield_now().await,
                    }
                }
            }).await.unwrap();
            for id in sc["bot_ids"].as_array().unwrap().iter().map(|id|id.to_string()) {
            let page=browser.get(&app,&format!("/bots/{id}")).await;
            assert_eq!(page.status,200,"{name}: {}",page.body);
            let selector=scraper::Selector::parse("#metrics").unwrap();
            let doc=scraper::Html::parse_document(&page.body);
            let got=doc.select(&selector).next().unwrap().html();
            let want=scraper::Html::parse_fragment(expected["bots"][&id]["metrics"].as_str().unwrap());
            let want=want.select(&selector).next().unwrap().html();
            assert_eq!(common::html::normalize(&got),common::html::normalize(&want),"warm route {name}");
            // Warm: the page's chart frame asks for nothing, and it arrives drawn.
            let chart=chart_frames(name,"warm",&id,&mut browser,&app,&expected["warm"]["bots"][&id]).await;
            assert!(chart.contains("data-controller=\"bot--chart\""));
            }
            dashboard(name,"warm",&mut browser,&app,&expected["warm"]["dashboard"]).await;
            // The scheduler records each job's run (an app_configs row) and its syncs touch the reference data, from a
            // connection of their own, all the time: neither is this account's figures, and the warm pages stay warm.
            let other=rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
            other.execute("INSERT INTO app_configs(key,value,created_at,updated_at) VALUES('rust_job.parity','{}',?1,?1)",[sc["at"].as_str().unwrap()]).unwrap();
            other.execute("UPDATE assets SET updated_at=?1",[sc["at"].as_str().unwrap()]).unwrap();
            drop(other);
            for id in sc["bot_ids"].as_array().unwrap().iter().map(|id|id.to_string()) { chart_frames(name,"warm",&id,&mut browser,&app,&expected["warm"]["bots"][&id]).await; }
            dashboard(name,"warm",&mut browser,&app,&expected["warm"]["dashboard"]).await;
            // Warm: a page that connected after the fill asks, and the answer is published at once.
            let first=sc["bot_ids"][0].as_i64().unwrap();
            for (path,body) in [("/en/broadcasts/metrics_update",json!({"bot_id":first})),("/en/broadcasts/pnl_update",json!({"bot_ids":sc["bot_ids"]})),
                                ("/en/broadcasts/global_pnl_update",json!({}))] {
                cable.drain(&app,&user_stream).await;
                let answer=browser.send_body(&app,"POST",path,Some(body.to_string()),common::web::Csrf::Header,&[("content-type","application/json")]).await;
                assert_eq!(answer.status,200,"{name} {path}");
                delivered(name,path,&cable.until(&wanted).await,&rails);
            }
            if name == "owner_account" {
            let index=browser.get(&app,"/bots").await;
            assert_eq!(index.status,200);
            let doc=scraper::Html::parse_document(&index.body);
            let global=scraper::Selector::parse("#global-pnl").unwrap();
            assert_eq!(common::html::normalize(&doc.select(&global).next().unwrap().html()),common::html::normalize(expected["account"].as_str().unwrap()),"warm account {name}");
            }
            assert_eq!(browser.get(&app,"/bots/999/chart").await.status,404);
            assert_eq!(common::web::Browser::default().get(&app,"/bots/1").await.status,302);
            if name == "owner_account" {
                let prepared=loading::prepare(&app,user).await.unwrap();
                let loading::Snapshot::Ready(mut refused,at,revision,_,until)=prepared.clone() else { panic!("not warm") };
                // A missing cache entry must never expose the core's successful ledger fallback.
                let pending=loading::Snapshot::Ready(Cache::default(),at,revision,None,until);
                let pending=app.db(move|c| loading::render(c,user,&pending,"en","token","",&deltabadger::engine::FixedClock(at.utc()))).await.unwrap().unwrap();
                let failed=app.db(move|c| loading::render(c,user,&loading::Snapshot::Failed(None),"en","token","",&deltabadger::engine::FixedClock(at.utc()))).await.unwrap().unwrap();
                assert_eq!(pending,failed,"pending demands published a ledger figure or account total");
                // Cache a refused price demand and require the same honest unavailable result.
                let names=app.db(move|c| Ok(loading::symbols(c,user).unwrap())).await.unwrap();
                let empty=Cache::default();
                let reader=Reader::new(&empty,at.utc().timestamp()).with_symbols(names.clone());
                use deltabadger::figures::market::{MarketData,Venue};
                let venue=Venue{exchange_id:1,exchange_type:"Exchanges::Alpaca".into()};
                assert!(reader.prices(&venue,&names).is_err());
                let demands=reader.demands();
                let wire=Wire{script:serde_json::json!({"GET data.alpaca.markets/v2/stocks/snapshots":{"status":503,"body":{}},"GET data.alpaca.markets/v1beta3/crypto/us/latest/trades":{"status":503,"body":{}}}),calls:RefCell::default()};
                refused.fill(&wire,demands,at.utc().timestamp()).await;
                let reader=Reader::new(&refused,at.utc().timestamp()).with_symbols(names);
                assert!(reader.prices(&venue,&[]).is_err());
                assert!(reader.failed());
                assert!(reader.demands().is_empty());
                let refused=loading::Snapshot::Ready(refused,at,revision,None,until);
                let refused=app.db(move|c| loading::render(c,user,&refused,"en","token","",&deltabadger::engine::FixedClock(at.utc()))).await.unwrap().unwrap();
                assert_eq!(refused,failed,"failed demand published a ledger figure or account total");
                // Keep successful prices, but replace every candle response with a failure or unreadable bar.
                for body in [serde_json::json!({"status":503,"body":{}}),
                             serde_json::json!({"body":"not JSON"}),
                             serde_json::json!({"body":{"bars":[{"t":"2026-03-02T00:00:00Z","o":null}]}})] {
                    let mut script = sc["script"].clone();
                    for (key, reply) in script.as_object_mut().unwrap() {
                        if key.contains("/bars") { *reply = body.clone(); }
                    }
                    let wire = Wire { script, calls: RefCell::default() };
                    let names = loading::symbols(&c,user).unwrap();
                    let mut cache = Cache::default();
                    for _ in 0..4 {
                        let reader = Reader::new(&cache, at.utc().timestamp()).with_symbols(names.clone());
                        let _ = deltabadger::web::figure::account(&c,user,&reader,at,"en","token","");
                        let demands = reader.demands();
                        if demands.is_empty() { break; }
                        cache.fill(&wire,demands,at.utc().timestamp()).await;
                    }
                    assert!(wire.calls.borrow().iter().any(|key| key.contains("/bars")));
                    let reader = Reader::new(&cache, at.utc().timestamp()).with_symbols(names);
                    let _ = deltabadger::web::figure::account(&c,user,&reader,at,"en","token","");
                    assert!(reader.demands().is_empty(), "publication must fail on cached candles, not pending reads");
                    assert!(reader.failed(), "cached candle failure escaped publication guard");
                    let snapshot = loading::Snapshot::Ready(cache,at,revision,None,until);
                    let rendered = app.db(move|c| loading::render(c,user,&snapshot,"en","token","",&deltabadger::engine::FixedClock(at.utc()))).await.unwrap().unwrap();
                    assert_eq!(rendered,failed,"failed candles published a chart or account sparkline");
                }
                // Index tickers use the venue's entire symbol union. Synchronization inserts
                // another listing after prepare; all dependent targets must lose their numbers. Whether the page's own
                // connection writes it or the sync's.
                let insert="INSERT INTO tickers (base,base_asset_id,base_decimals,created_at,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,updated_at) SELECT 'USD_TEST',quote_asset_id,base_decimals,created_at,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,'USD_TEST',updated_at FROM tickers WHERE ticker='AAA' LIMIT 1";
                for own in [true,false] {
                    let prepared=loading::prepare(&app,user).await.unwrap();
                    assert!(matches!(prepared,loading::Snapshot::Ready(..)));
                    if own { app.db(move|c| { c.execute(insert,[])?; Ok(()) }).await.unwrap(); }
                    else { rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap().execute(insert,[]).unwrap(); }
                    let changed=app.db(move|c| {
                        assert!(loading::symbols(c,user).unwrap().iter().any(|s|s=="USD_TEST"));
                        loading::render(c,user,&prepared,"en","token","",&deltabadger::engine::FixedClock(at.utc()))
                    }).await.unwrap().unwrap();
                    assert_eq!(changed,failed,"new ticker published a ledger figure or account total (written by the page's connection: {own})");
                    app.db(|c| { c.execute("DELETE FROM tickers WHERE ticker='USD_TEST'",[])?; Ok(()) }).await.unwrap();
                }
                // Kept figures are what a computation gives: rendered again under another form token, they equal a fresh
                // computation under it. Stored text that holds what a kept page once used as its token's placeholder is
                // text: it reaches the page as stored and never carries the request's token (an image URL would send it
                // to the image's host).
                let marker="https://images.example/figuresFormTokenPlaceholder7f3a9c.png";
                let aaa="UPDATE assets SET image_url=?1 WHERE id=(SELECT base_asset_id FROM tickers WHERE ticker='AAA')";
                let original:Option<String>=c.query_row("SELECT image_url FROM assets WHERE id=(SELECT base_asset_id FROM tickers WHERE ticker='AAA')",[],|r|r.get(0)).unwrap();
                app.db(move|c| { c.execute(aaa,[marker])?; Ok(()) }).await.unwrap();
                let warm=|app:App|async move { tokio::time::timeout(Duration::from_secs(10),async { loop {
                    match loading::prepare(&app,user).await.unwrap() { ready@loading::Snapshot::Ready(..)=>break ready, _=>tokio::time::sleep(Duration::from_millis(20)).await }
                } }).await.expect("the figures never became ready again") };
                let prepared=warm(app.clone()).await;
                let loading::Snapshot::Ready(cache,at,..)=prepared.clone() else { panic!("not warm") };
                for token in ["first-token","second-token"] {
                    let snapshot=prepared.clone();
                    let kept=app.db(move|c| loading::render(c,user,&snapshot,"en",token,"",&deltabadger::engine::FixedClock(at.utc()))).await.unwrap().unwrap();
                    let reader=Reader::new(&cache,at.utc().timestamp()).with_current(&c).with_symbols(loading::symbols(&c,user).unwrap());
                    assert_eq!(kept,deltabadger::web::figure::account(&c,user,&reader,at,"en",token,"").unwrap(),"kept figures under {token}");
                    assert!(kept.to_string().contains(marker) && !kept.to_string().contains(&format!("images.example/{token}")),"the stored image URL is shown as stored");
                }
                app.db(move|c| { c.execute(aaa,[original])?; Ok(()) }).await.unwrap();
                warm(app.clone()).await;
            }
            if name == "owner_account" {
                // A large account's publication does not outrun the hub: 600 payloads, two hundred bots' worth, all reach
                // an open connection, which stays open.
                let many:Vec<(String,String)>=(0..600).map(|i|(user_stream.clone(),format!("<turbo-stream action=\"replace\" target=\"many-{i}\"><template></template></turbo-stream>"))).collect();
                let keys:BTreeSet<(String,String)>=many.iter().map(|(stream,html)|(stream.clone(),target(html))).collect();
                cable.drain(&app,&user_stream).await;
                loading::deliver(&app,many).await;
                assert_eq!(cable.until(&keys).await.len(),600,"a connection fell behind the hub");
                cable.drain(&app,&user_stream).await;
                // Market data that fails after the failure window has run out: the wait ends, with no value.
                let mut failing=sc["script"].clone();
                for (key,reply) in failing.as_object_mut().unwrap() { if key.contains("/snapshots") { *reply=json!({"status":503,"body":{},"delay_ms":300}); } }
                let late=common::web::TestClock::at(sc["at"].as_str().unwrap());
                let broken=App::new(Config::from_env(&env).unwrap(),&env,rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap(),late.clone()).unwrap().with_figure_source(Source::Script(failing)).unwrap();
                let mut watching=Cable::open(&broken,&cookie,&streams).await;
                assert!(matches!(loading::prepare(&broken,user).await.unwrap(),loading::Snapshot::Cold));
                loading::publish(&broken,user).await.unwrap();
                late.set(now.utc()+chrono::Duration::seconds(120)); // the fill ends two minutes after it began
                let ended=watching.until(&wanted).await;
                assert_eq!(ended.len(),wanted.len(),"{name}: a fill that failed late never ended the wait");
                assert!(ended.values().all(|html|html.contains("no-value")),"{name}: a failed fill published a figure");
                // A page that stays mounted while its connection drops the final publication: its mailbox is past its
                // bounds, so the publication is dropped and the connection closed. The page reconnects and subscribes
                // again, asks nothing (`broadcast--on-connect` asks once), and ends with the current figures: replayed
                // while they are fresh, and refreshed once they expired, after a restart, after any replay miss.
                // Market answers are held back 300 ms, so the fill its first subscription asks for is still running when
                // its mailbox overflows.
                let open_app=|clock:std::sync::Arc<common::web::TestClock>| App::new(Config::from_env(&env).unwrap(),&env,rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap(),clock).unwrap().with_figure_source(Source::Script(held.clone())).unwrap();
                let clock=common::web::TestClock::at(sc["at"].as_str().unwrap());
                let mounted_app=open_app(clock.clone());
                let mut mounted=Cable::open(&mounted_app,&cookie,&streams).await;
                mounted_app.hub.broadcast(&user_stream,&"x".repeat(deltabadger::web::cable::MAILBOX_BYTES+1));
                tokio::time::timeout(Duration::from_secs(5),async {
                    while !matches!(loading::prepare(&mounted_app,user).await.unwrap(),loading::Snapshot::Ready(..)) { tokio::time::sleep(Duration::from_millis(20)).await; }
                }).await.unwrap();
                loading::publish(&mounted_app,user).await.unwrap(); // the final publication, which the full mailbox drops
                let closed=tokio::time::timeout(Duration::from_secs(5),async {
                    use futures_util::StreamExt;
                    while let Some(Ok(message))=mounted.socket.next().await { assert!(!message.to_text().unwrap_or_default().contains("turbo-stream"),"{name}: a dropped publication arrived"); }
                }).await;
                assert!(closed.is_ok(),"{name}: a connection past its mailbox's bounds stayed open");
                drop(mounted);
                let kept=mounted_app.figure_service.settled(user);
                let mut again=Cable::open(&mounted_app,&cookie,&streams).await;
                let replayed=again.until(&wanted).await;
                delivered(name,"a reconnect after a dropped publication",&replayed,&rails);
                assert_eq!(mounted_app.figure_service.settled(user),kept,"{name}: fresh figures were refilled, not sent again");
                drop(again);
                // Idle past the cache's five minutes: nothing stale is sent; a refresh is filled and published, every target
                // of it, and the charts now end five minutes later than the replayed ones (Rails recorded neither).
                clock.set(now.utc()+chrono::Duration::seconds(301));
                let mut later=Cable::open(&mounted_app,&cookie,&streams).await;
                let refreshed=later.until(&wanted).await;
                assert_eq!(refreshed.keys().collect::<Vec<_>>(),wanted.iter().collect::<Vec<_>>(),"{name}: a reconnect after the figures expired");
                assert_ne!(mounted_app.figure_service.settled(user),kept,"{name}: expired figures were sent again, not refreshed");
                assert_ne!(refreshed,replayed,"{name}: the expired figures were sent again");
                // A publication held at the database while its figures expire delivers nothing: it asks for a refresh,
                // whose figures arrive instead.
                later.drain(&mounted_app,&user_stream).await;
                let held_serial=mounted_app.figure_service.settled(user);
                let publishing={ let app=mounted_app.clone(); tokio::spawn(async move { loading::publish(&app,user).await }) };
                tokio::task::yield_now().await; // the publication has asked for the account; now the database is held
                let busy=mounted_app.clone();
                let hold=tokio::spawn(async move { busy.db(|_| { std::thread::sleep(std::time::Duration::from_millis(1500)); Ok(()) }).await });
                tokio::time::sleep(Duration::from_millis(500)).await;
                clock.set(now.utc()+chrono::Duration::seconds(602));
                hold.await.unwrap().unwrap();
                publishing.await.unwrap().unwrap();
                let renewed=later.until(&wanted).await;
                assert_eq!(renewed.len(),wanted.len(),"{name}: no figures after a publication its expiry overtook");
                assert_ne!(renewed,refreshed,"{name}: a publication delivered figures that expired while it was held");
                assert_ne!(mounted_app.figure_service.settled(user),held_serial,"{name}: no refresh after an expired publication");
                drop(later);
                // A restart (an eviction leaves the service the same way): nothing is kept, and the subscription asks.
                let restarted=open_app(common::web::TestClock::at(sc["at"].as_str().unwrap()));
                let mut after=Cable::open(&restarted,&cookie,&streams).await;
                delivered(name,"a reconnect after a restart",&after.until(&wanted).await,&rails);
            }
            // An order the engine wrote publishes its owner's figures again, from a fill of their own: the write moved the
            // revision (here it is written and put back, so the figures are Rails' again), and the event alone is the trigger.
            if name == "owner_account" {
                let (events, heard) = tokio::sync::mpsc::unbounded_channel();
                let (stopper, stop) = tokio::sync::watch::channel(false);
                let follower = tokio::spawn(loading::follow(app.clone(), heard, stop));
                app.db(|c| { c.execute("UPDATE transactions SET quote_amount_exec=quote_amount_exec+1 WHERE id=(SELECT min(id) FROM transactions)",[])?;
                             c.execute("UPDATE transactions SET quote_amount_exec=quote_amount_exec-1 WHERE id=(SELECT min(id) FROM transactions)",[])?; Ok(()) }).await.unwrap();
                cable.drain(&app,&user_stream).await;
                events.send(EngineEvent::OrderUpdated { bot_id: sc["bot_ids"][0].as_i64().unwrap() }).unwrap();
                delivered(name,"an order the engine wrote",&cable.until(&wanted).await,&rails);
                // A burst of orders is one publication per account, not one per order.
                cable.drain(&app,&user_stream).await;
                let before=cable.frames;
                for bot in sc["bot_ids"].as_array().unwrap().iter().cycle().take(60) {
                    events.send(EngineEvent::OrderRecorded { bot_id: bot.as_i64().unwrap(), transaction_id: 1 }).unwrap();
                }
                delivered(name,"a burst of orders",&cable.until(&wanted).await,&rails);
                tokio::time::sleep(Duration::from_secs(1)).await;
                cable.drain(&app,&user_stream).await;
                assert_eq!(cable.frames-before,wanted.len(),"{name}: sixty orders published more than once");
                // Orders keep coming while a publication is held up (the web connection busy for 1.5 s): they are
                // marked as they come, one mark per bot, and the account publishes once more afterwards.
                let before=cable.frames;
                let busy=app.clone();
                let hold=tokio::spawn(async move { busy.db(|_| { std::thread::sleep(std::time::Duration::from_millis(1500)); Ok(()) }).await });
                let bots:Vec<i64>=sc["bot_ids"].as_array().unwrap().iter().map(|bot|bot.as_i64().unwrap()).collect();
                events.send(EngineEvent::OrderRecorded { bot_id: bots[0], transaction_id: 1 }).unwrap();
                tokio::time::sleep(Duration::from_millis(700)).await; // that burst is publishing now, held at the database
                for bot in bots.iter().cycle().take(3000) {
                    events.send(EngineEvent::OrderRecorded { bot_id: *bot, transaction_id: 1 }).unwrap();
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                assert_eq!(app.figure_service.marked(),bots.len(),"{name}: orders waited behind a publication instead of being marked");
                hold.await.unwrap().unwrap();
                delivered(name,"orders during a publication",&cable.until(&wanted).await,&rails);
                tokio::time::sleep(Duration::from_millis(1500)).await;
                cable.drain(&app,&user_stream).await;
                assert_eq!(cable.frames-before,2*wanted.len(),"{name}: the held publication and one more for the orders behind it");
                // A publication that fails on a transient database error keeps its demand: here the bots table is away
                // while the order's mark is published, and back a moment later. The page receives the figures with no
                // other trigger, once a retry finds the database whole again.
                cable.drain(&app,&user_stream).await;
                app.db(|c| { c.execute_batch("ALTER TABLE bots RENAME TO bots_away")?; Ok(()) }).await.unwrap();
                events.send(EngineEvent::OrderUpdated { bot_id: sc["bot_ids"][0].as_i64().unwrap() }).unwrap();
                tokio::time::sleep(Duration::from_millis(1200)).await; // its first publication has failed
                app.db(|c| { c.execute_batch("ALTER TABLE bots_away RENAME TO bots")?; Ok(()) }).await.unwrap();
                delivered(name,"a publication retried after a database error",&cable.until(&wanted).await,&rails);
                // A publication that fails at the end of the fill it waited for (the user's locale column is away for a
                // moment: the fill never reads it, the publication does) is handed back to this service and retried: the
                // page receives the figures with no other trigger.
                cable.drain(&app,&user_stream).await;
                app.db(|c| { c.execute("UPDATE transactions SET quote_amount_exec=quote_amount_exec+1 WHERE id=(SELECT min(id) FROM transactions)",[])?;
                             c.execute("UPDATE transactions SET quote_amount_exec=quote_amount_exec-1 WHERE id=(SELECT min(id) FROM transactions)",[])?;
                             c.execute_batch("ALTER TABLE users RENAME COLUMN locale TO locale_away")?; Ok(()) }).await.unwrap();
                events.send(EngineEvent::OrderUpdated { bot_id: sc["bot_ids"][0].as_i64().unwrap() }).unwrap();
                tokio::time::sleep(Duration::from_millis(1500)).await; // the write's fill has ended, and its publication failed
                app.db(|c| { c.execute_batch("ALTER TABLE users RENAME COLUMN locale_away TO locale")?; Ok(()) }).await.unwrap();
                delivered(name,"a publication retried after the end of its fill",&cable.until(&wanted).await,&rails);
                // A publication whose reads of the bots fail once (the table is away while a page asks; the render's fallback
                // fails first, and stream construction would): nothing is recorded or delivered, the demand is kept and
                // retried, and once the table is back the mounted page and a page that connects afresh both end with every
                // figure; nothing complete and empty was kept to replay.
                cable.drain(&app,&user_stream).await;
                app.db(|c| { c.execute_batch("ALTER TABLE bots RENAME TO bots_away")?; Ok(()) }).await.unwrap();
                let asked=browser.send_body(&app,"POST","/en/broadcasts/global_pnl_update",Some(json!({}).to_string()),common::web::Csrf::Header,&[("content-type","application/json")]).await;
                assert_eq!(asked.status,200,"{name}: the request is answered whatever its publication does");
                tokio::time::sleep(Duration::from_millis(1200)).await; // its publication has failed, and a retry with it
                app.db(|c| { c.execute_batch("ALTER TABLE bots_away RENAME TO bots")?; Ok(()) }).await.unwrap();
                delivered(name,"a publication retried after its streams failed",&cable.until(&wanted).await,&rails);
                let mut afresh=Cable::open(&app,&cookie,&streams).await;
                delivered(name,"a reconnect after a publication whose streams failed",&afresh.until(&wanted).await,&rails);
                drop(afresh);
                // A subscription after a missed fill, while the key's credentials cannot be read (their column is away): the
                // read fails rather than meaning "no figures", the demand is kept and retried, and once the column is back the
                // page that subscribed ends with every figure, with no other request or event.
                app.db(|c| { c.execute("UPDATE transactions SET quote_amount_exec=quote_amount_exec+1 WHERE id=(SELECT min(id) FROM transactions)",[])?;
                             c.execute("UPDATE transactions SET quote_amount_exec=quote_amount_exec-1 WHERE id=(SELECT min(id) FROM transactions)",[])?;
                             c.execute_batch("ALTER TABLE api_keys RENAME COLUMN passphrase TO passphrase_away")?; Ok(()) }).await.unwrap();
                let mut missed=Cable::open(&app,&cookie,&streams).await;
                tokio::time::sleep(Duration::from_millis(1200)).await; // its subscription's read has failed, and a retry with it
                app.db(|c| { c.execute_batch("ALTER TABLE api_keys RENAME COLUMN passphrase_away TO passphrase")?; Ok(()) }).await.unwrap();
                delivered(name,"a subscription retried after its credentials could not be read",&missed.until(&wanted).await,&rails);
                drop(missed);
                // A fallback render (its fill failed) whose read of the account's bots fails once: no publication of the
                // headline alone is accepted, the demand is kept and retried, and once the bots are back the page ends with
                // every target's no-value markup.
                let mut failing=sc["script"].clone();
                for (key,reply) in failing.as_object_mut().unwrap() { if key.contains("/snapshots") { *reply=json!({"status":503,"body":{},"delay_ms":300}); } }
                let late=common::web::TestClock::at(sc["at"].as_str().unwrap());
                let broken=App::new(Config::from_env(&env).unwrap(),&env,rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap(),late.clone()).unwrap().with_figure_source(Source::Script(failing)).unwrap();
                let (_quiet,no_events)=tokio::sync::mpsc::unbounded_channel(); // held, so its `figures` service keeps running
                let (stop_broken,broken_stop)=tokio::sync::watch::channel(false);
                let broken_follower=tokio::spawn(loading::follow(broken.clone(),no_events,broken_stop));
                let mut watching=Cable::open(&broken,&cookie,&streams).await; // its subscription asks; the fill fails
                app.db(|c| { c.execute_batch("ALTER TABLE bots RENAME TO bots_away")?; Ok(()) }).await.unwrap();
                // Check the fallback's own boundary: the later stream lookup can also fail and mask a swallowed read.
                app.db(move |c| {
                    assert!(loading::render(c,user,&loading::Snapshot::Failed(None),"en","","",&deltabadger::engine::FixedClock(chrono::Utc::now())).is_err(),
                        "a fallback whose bots cannot be read must fail before constructing a partial page");
                    Ok(())
                }).await.unwrap();
                late.set(now.utc()+chrono::Duration::seconds(120)); // the fill ends two minutes after it began
                tokio::time::sleep(Duration::from_millis(1200)).await; // its publication's fallback has failed, and a retry with it
                app.db(|c| { c.execute_batch("ALTER TABLE bots_away RENAME TO bots")?; Ok(()) }).await.unwrap();
                let ended=watching.until(&wanted).await;
                assert_eq!(ended.len(),wanted.len(),"{name}: a fallback whose bots could not be read published part of the page");
                assert!(ended.values().all(|html|html.contains("no-value")),"{name}: a failed fill published a figure");
                drop(watching);
                stop_broken.send(true).unwrap();
                assert_eq!(tokio::time::timeout(Duration::from_secs(5),broken_follower).await.unwrap().unwrap(),Ok(()));
                // The engine's own path, on a copy of this database (Decision 9): a publication has read, rendered and
                // checked the account and is held just before it delivers while a real follow-up poll commits a terminal fill
                // of a stopped bot, whose row carries a malformed `updated_at`. The poll announces the bot unconditionally,
                // its mark brings a later publication, and the page ends on the figures after the fill with no request of
                // its own. The copy's
                // bots are stopped and its other open orders closed, so the engine polls that order only: a basket's, as the
                // engine takes outstanding work only from a bot it can run (a regular basket, no rebalance or liquidation).
                let copy=tempfile::tempdir().unwrap();
                let primary=copy.path().join("production.sqlite3");
                app.db({ let primary=primary.to_string_lossy().to_string(); move|c| { c.execute("VACUUM INTO ?1",[primary])?; Ok(()) } }).await.unwrap();
                rusqlite::Connection::open(dir.join("production_queue.sqlite3")).unwrap()
                    .execute("VACUUM INTO ?1",[copy.path().join("production_queue.sqlite3").to_string_lossy().to_string()]).unwrap();
                let (tx,ext,symbol,notional,qty,price):(i64,String,String,String,String,String)={
                    let c=rusqlite::Connection::open(&primary).unwrap();
                    c.execute("UPDATE bots SET status=2",[]).unwrap();
                    c.execute("UPDATE transactions SET external_status=2 WHERE status=0 AND external_status IN (0,1)",[]).unwrap();
                    let order=c.query_row("SELECT t.id,t.external_id,coalesce(t.base,''),coalesce(CAST(t.quote_amount AS TEXT),''),CAST(t.amount_exec AS TEXT),CAST(t.price AS TEXT) \
                        FROM transactions t JOIN bots b ON b.id=t.bot_id WHERE b.user_id=?1 AND b.type='Bots::DcaMultiAsset' AND t.transaction_type='REGULAR' \
                        AND NOT EXISTS (SELECT 1 FROM transactions o WHERE o.bot_id=b.id AND o.transaction_type!='REGULAR') AND t.status=0 AND t.external_status=2 AND t.amount_exec>0 AND t.price>0 \
                        AND t.external_id IS NOT NULL ORDER BY t.id DESC LIMIT 1",[user],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).unwrap();
                    c.execute("UPDATE transactions SET external_status=1,amount_exec=NULL,quote_amount_exec=NULL WHERE id=?1",[order.0]).unwrap();
                    // A malformed `updated_at` (an integer, not a time) is there before the poll: nothing the engine reads to
                    // announce may stand between the poll's write and the bot's announcement.
                    c.execute("UPDATE transactions SET updated_at=0 WHERE id=?1",[order.0]).unwrap();
                    order
                };
                let mut script=serde_json::Map::new();
                script.insert(format!("GET /v2/orders/{ext}"),json!([{ "status": 200, "body": { "id": ext, "status": "filled", "symbol": symbol, "type": "market", "side": "buy",
                    "notional": (!notional.is_empty()).then_some(notional), "qty": null, "filled_qty": qty, "filled_avg_price": price, "limit_price": null } }]));
                let at:chrono::DateTime<chrono::Utc>=sc["at"].as_str().unwrap().parse().unwrap();
                let cipher=deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&env,common::web::SECRET).unwrap());
                // The barrier: the armed clock holds the publication's second reading, taken after it rendered and checked
                // the account and just before it delivers, until the engine, on a thread of its own, has committed the
                // fill. Nothing the publication does can see the fill: only the engine's announcement brings the page to it.
                let gate=std::sync::Arc::new(Gate { clock: common::web::TestClock::at(sc["at"].as_str().unwrap()), armed: Default::default() });
                let (read,arrived)=std::sync::mpsc::channel::<()>();
                let (resume,go)=std::sync::mpsc::channel::<()>();
                let (receiver,events_from_engine)=std::sync::mpsc::channel();
                // The engine lives until the test is done with it: its events' channel closing would end `follow`'s reading.
                let (done,engine_done)=std::sync::mpsc::channel::<bool>();
                let (finish,finished)=std::sync::mpsc::channel::<()>();
                let copy_dir=copy.path().to_path_buf();
                let engine=std::thread::spawn(move || {
                    let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                    rt.block_on(async move {
                        let paths=deltabadger::store::Paths::from_env(&|_| None,&copy_dir);
                        let mut engine=deltabadger::engine::run::Engine::new(deltabadger::store::open(&paths).unwrap().primary,
                            Scripted(deltabadger::venue::http::ScriptedTransport::from_script(&Value::Object(script))),cipher,deltabadger::lease::lock(&paths,at).unwrap());
                        receiver.send(engine.subscribe_to(EngineEvent::is_order)).unwrap();
                        arrived.recv().unwrap(); // the publication has rendered and checked the account, and waits at the clock
                        // The engine's first step takes over the install; a later one polls every outstanding order.
                        let filled=|engine:&deltabadger::engine::run::Engine<Scripted>| engine.primary.query_row("SELECT external_status FROM transactions WHERE id=?1",[tx],|r| r.get::<_,i64>(0)).unwrap()==2;
                        for step in 0..6 {
                            if filled(&engine) { break; }
                            deltabadger::engine::run::step(&mut engine,&deltabadger::engine::FixedClock(at+chrono::Duration::seconds(60+10*step))).await.unwrap();
                        }
                        done.send(filled(&engine)).unwrap();
                        resume.send(()).unwrap();
                        let _ = finished.recv();
                    })
                });
                // No ping or recheck reads the clock while the gate is armed: only the publication does.
                let copy_app=App::new(Config::from_env(&env).unwrap(),&env,rusqlite::Connection::open(&primary).unwrap(),gate.clone()).unwrap()
                    .with_figure_source(Source::Script(held.clone())).unwrap().with_cable_timing(Duration::from_secs(3600),Duration::from_secs(3600)).unwrap();
                let (stop_copy,copy_stop)=tokio::sync::watch::channel(false);
                let copy_follower=tokio::spawn(loading::follow(copy_app.clone(),events_from_engine.recv().unwrap(),copy_stop));
                let mut page=Cable::open(&copy_app,&cookie,&streams).await;
                let before=page.until(&wanted).await; // its subscription asked: the figures before the fill
                assert_eq!(before.len(),wanted.len(),"{name}: the copy's figures before the fill");
                *gate.armed.lock().unwrap()=Some((1,read,go));
                loading::publish(&copy_app,user).await.unwrap(); // reads, renders and checks; waits while the engine commits; delivers
                assert!(gate.armed.lock().unwrap().is_none(),"{name}: the publication never reached the barrier");
                assert!(engine_done.recv().unwrap(),"{name}: the engine's poll committed the fill");
                let truth_app=App::new(Config::from_env(&env).unwrap(),&env,rusqlite::Connection::open(&primary).unwrap(),common::web::TestClock::at(sc["at"].as_str().unwrap())).unwrap().with_figure_source(Source::Script(held.clone())).unwrap();
                let truth=Cable::open(&truth_app,&cookie,&streams).await.until(&wanted).await;
                assert_eq!(truth.len(),wanted.len(),"{name}: a fresh process published the figures after the fill");
                assert_ne!(truth,before,"{name}: the fill changed no figure");
                let mut seen=BTreeMap::new();
                let converged=tokio::time::timeout(Duration::from_secs(10),async {
                    while !wanted.iter().all(|key| seen.get(key)==truth.get(key)) {
                        let value=page.next().await;
                        let (Some(identifier),Some(html))=(value["identifier"].as_str(),value["message"].as_str()) else { continue };
                        seen.insert((page.streams[identifier].clone(),target(html)),html.to_string());
                    }
                }).await;
                assert!(converged.is_ok(),"{name}: the page did not end on the figures after the fill");
                tokio::time::sleep(Duration::from_secs(1)).await;
                copy_app.hub.broadcast(&user_stream,"settled");
                loop {
                    let value=page.next().await;
                    if value["message"]=="settled" { break; }
                    let (Some(identifier),Some(html))=(value["identifier"].as_str(),value["message"].as_str()) else { continue };
                    seen.insert((page.streams[identifier].clone(),target(html)),html.to_string());
                }
                assert!(wanted.iter().all(|key| seen.get(key)==truth.get(key)),"{name}: an older publication came after the figures of the fill");
                finish.send(()).unwrap();
                engine.join().unwrap();
                stop_copy.send(true).unwrap();
                assert_eq!(tokio::time::timeout(Duration::from_secs(5),copy_follower).await.unwrap().unwrap(),Ok(()));
                stopper.send(true).unwrap();
                assert_eq!(tokio::time::timeout(Duration::from_secs(5),follower).await.unwrap().unwrap(),Ok(()));
            }
            // A correction to an existing fill must invalidate without a new transaction id.
            if name == "owner_account" {
                let original=app.db(|c| Ok(c.query_row("SELECT quote_amount_exec FROM transactions ORDER BY id LIMIT 1",[],|r|r.get::<_,rusqlite::types::Value>(0))?)).await.unwrap();
                app.db(|c| { c.execute("UPDATE transactions SET quote_amount_exec=quote_amount_exec+1 WHERE id=(SELECT min(id) FROM transactions)",[])?; Ok(()) }).await.unwrap();
                assert!(matches!(loading::prepare(&app,user).await.unwrap(),loading::Snapshot::Cold));
                tokio::time::timeout(std::time::Duration::from_secs(5),async {
                    loop {
                        match loading::prepare(&app,user).await.unwrap() {
                            loading::Snapshot::Ready(..)=>break,
                            loading::Snapshot::Failed(_)=>panic!("corrected fill failed"),
                            _=>tokio::task::yield_now().await,
                        }
                    }
                }).await.unwrap();
                app.db(move|c| { c.execute("UPDATE transactions SET quote_amount_exec=?1 WHERE id=(SELECT min(id) FROM transactions)",[original])?; Ok(()) }).await.unwrap();
            }

        }
        if name == "split_fresh" {
            unreadable_split_report(&dir, &sc, &expected, now, name).await;
            let later = now.plus_seconds(3 * 86_400).unwrap();
            let mut after = sc.clone();
            after["at"] = json!(later.utc().to_rfc3339());
            unreadable_split_report(&dir, &after, &expected, later, "split_quarantine_over").await;
        }
        let reader = Reader::new(&cache, now.utc().timestamp());
        let actual = deltabadger::web::figure::account(&c, sc["user_id"].as_i64().unwrap(), &reader, now, "en", "token", "").unwrap();
        if ["split_fresh", "split_unsized"].contains(&name) {
            for parts in actual["bots"].as_object().unwrap().values() {
                for part in ["tile", "metrics", "chart"] {
                    let html = parts[part].as_str().unwrap();
                    assert!(html.contains("no-value"), "{name}: stale {part} published");
                    assert!(!html.contains("data-bot--chart-series"));
                    assert!(!html.contains("rbutton--success"));
                }
            }
            let metrics = scraper::Html::parse_fragment(actual["bots"]["1"]["metrics"].as_str().unwrap());
            let rows = metrics.select(&scraper::Selector::parse("tr[data-symbol]").unwrap()).collect::<Vec<_>>();
            assert!(!rows.is_empty(), "{name}: known quantities disappeared");
            for row in rows {
                assert_eq!(row.select(&scraper::Selector::parse(".no-value").unwrap()).count(), 2, "{name}: stale holding value or P/L published");
            }
            assert!(actual["account"].as_str().unwrap().contains("no-value"), "{name}: stale account published");
            assert!(!actual["account"].as_str().unwrap().contains("<svg"), "{name}: stale sparkline published");
            continue; // Deliberate unavailable state; Rails retains stale ledger figures here.
        }
        if name.starts_with("locked_") {
            assert!(expected["bots"]["1"]["metrics"].as_str().unwrap().contains("wash_sale_table"), "{name}: Rails did not record a lock");
        }
        if name == "index_specials" {
            // The bot page of an index bot with liquidation, redeploy and rebalance legs, once its figures are filled: served,
            // with the metrics Rails rendered (less the redeploy refusal this build adds, as for the fragments below).
            use deltabadger::web::{App,Config,session::{self,SessionData},figure::loading::{self,Source}};
            let env=common::web::env(common::web::SECRET);
            let app=App::new(Config::from_env(&env).unwrap(),&env,rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap(),common::web::TestClock::at(sc["at"].as_str().unwrap()))
                .unwrap().with_figure_source(Source::Script(sc["script"].clone())).unwrap();
            let user=sc["user_id"].as_i64().unwrap();
            let password:String=c.query_row("SELECT encrypted_password FROM users WHERE id=?1",[user],|r|r.get(0)).unwrap();
            let signed=SessionData { user:Some((user,password.chars().take(29).collect())),..SessionData::default() };
            let mut browser=common::web::Browser { cookie:Some(session::seal(&app.keys.session,&signed,now.utc())),page:None };
            loading::publish(&app,user).await.unwrap();
            tokio::time::timeout(Duration::from_secs(10),async { loop {
                match loading::prepare(&app,user).await.unwrap() {
                    loading::Snapshot::Ready(..)=>break,
                    loading::Snapshot::Failed(why)=>panic!("{name}: fill failed: {why:?}"),
                    _=>tokio::task::yield_now().await,
                }
            } }).await.unwrap();
            let id=sc["bot_ids"][0].to_string();
            let page=browser.get(&app,&format!("/bots/{id}")).await;
            assert_eq!(page.status,200,"{name}: {}",page.body);
            let selector=scraper::Selector::parse("#metrics").unwrap();
            let got=scraper::Html::parse_document(&page.body).select(&selector).next().unwrap().html().replacen("<p role=\"status\">Redeploy unavailable</p>\n","",1);
            let want=scraper::Html::parse_fragment(expected["bots"][&id]["metrics"].as_str().unwrap());
            let want=want.select(&selector).next().unwrap().html();
            assert_eq!(common::html::normalize(&got),common::html::normalize(&want),"{name}: GET /bots/{id}");
        }
        if name == "price_untraded" {
            let document=scraper::Html::parse_fragment(actual["bots"]["1"]["metrics"].as_str().unwrap());
            let row=document.select(&scraper::Selector::parse("tr[data-symbol=BBB]").unwrap()).next().unwrap();
            assert_eq!(row.select(&scraper::Selector::parse(".no-value").unwrap()).count(),2);
            assert_eq!(row.select(&scraper::Selector::parse(".rbutton--success").unwrap()).count(),0);
            assert!(row.text().collect::<String>().contains("1.464973"));
            assert!(!row.text().collect::<String>().contains("-100"));
            assert!(!actual["bots"]["1"]["chart"].as_str().unwrap().contains("data-bot--chart-series"));
        }
        if std::env::var("FIGURE_BROADCASTS").as_deref() != Ok("0") {
            let streams=deltabadger::web::figure::streams(&c,sc["user_id"].as_i64().unwrap(),&actual).unwrap();
            for (stream,payload) in streams {
                let document=scraper::Html::parse_fragment(&payload);
                let selector=scraper::Selector::parse("turbo-stream").unwrap();
                let target=document.select(&selector).next().unwrap().value().attr("target").unwrap();
                let wanted=expected["broadcasts"].as_array().unwrap().iter().find(|entry| {
                    entry[0]==stream && entry[1].as_str().is_some_and(|s| s.contains(&format!("target=\"{target}\"")))
                }).unwrap_or_else(||panic!("{name}: no Rails broadcast to {stream} {target}"));
                if ["price_untraded", "index_rotation", "old_rows"].contains(&name) { continue; }
                let payload=payload.replace("<p role=\"status\">Redeploy unavailable</p>\n","");
                assert_eq!(common::html::normalize(&payload),common::html::normalize(wanted[1].as_str().unwrap()),"{name} broadcast {stream} {target}");
            }
        }
        let components = std::env::var("FIGURE_PARTS").unwrap_or_else(|_| "tile,metrics,chart,account".into());
        if components.split(',').any(|p| p == "account") {
            let got = actual["account"].as_str().unwrap();
            if ["price_untraded", "index_rotation", "old_rows"].contains(&name) { assert!(got.contains("no-value")); }
            else { assert_eq!(common::html::normalize(got),common::html::normalize(expected["account"].as_str().unwrap()),"{name} account"); }
        }
        for (id, parts) in expected["bots"].as_object().unwrap() {
            for part in components.split(',').filter(|p| *p != "account") {
                let got = &actual["bots"][id][part];
                if name == "price_untraded" || name == "index_rotation" || name == "old_rows" { assert!(got.as_str().unwrap().contains("no-value"), "{name} {part}"); continue; }
                let mut got = got.as_str().unwrap().to_string();
                if ["stranded_offset", "liquidations", "index_specials"].contains(&name) && part == "metrics" {
                    let refusal = "<p role=\"status\">Redeploy unavailable</p>\n";
                    assert!(got.contains(refusal));
                    assert_eq!(expected["offset"][0], expected["offset"][1], "{name}: a Rails read keeps the saved decline");
                    got = got.replacen(refusal, "", 1);
                }
                assert_eq!(common::html::normalize(&got), common::html::normalize(parts[part].as_str().unwrap()), "{name} bot {id} {part}");
            }
        }
    }
    assert_eq!(currencies, 108, "the display-currency scenarios and the sign boundaries (page_figures.rb)");
}

/// Denomination#format at the edges of its sign, as Rails wrote each (page_figures.rb#boundaries).
fn boundaries(dir: &Path) {
    let cases: Vec<(String, String, String, usize, String)> = serde_json::from_str(&std::fs::read_to_string(dir.join("rails.json")).unwrap()).unwrap();
    assert!(cases.len() > 100);
    let wrong: Vec<String> = cases.iter().filter_map(|(currency, rate, usd, digits, rails)| {
        let ours = deltabadger::web::figure::denominated(currency, rate, usd, *digits).unwrap();
        (&ours != rails).then(|| format!("{currency} {usd} at {digits}: Rails {rails}, ours {ours}"))
    }).collect();
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// The account's figures in the currency it is shown in, as the server keeps and publishes them: filled through the
/// figures service, with every conversion fetched from the scripted provider by the fill (none by the page), against
/// what Rails rendered and broadcast for the same install. Rails converts the tiles and the headline; the bot pages'
/// panels stay in each bot's quote currency, so they are compared as they are.
async fn display_currency(name: &str, dir: &Path, sc: &Value, expected: &Value) {
    use deltabadger::web::{App, Config, figure::loading::{self, Snapshot, Source}};
    let env = common::web::env(common::web::SECRET);
    let at = sc["at"].as_str().unwrap();
    let app = App::new(Config::from_env(&env).unwrap(), &env, rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap(), common::web::TestClock::at(at)).unwrap()
        .with_figure_source(Source::Script(sc["script"].clone())).unwrap();
    let user = sc["user_id"].as_i64().unwrap();
    let snapshot = tokio::time::timeout(Duration::from_secs(20), async { loop {
        match loading::prepare(&app, user).await.unwrap() { Snapshot::Ready(..) => break loading::prepare(&app, user).await.unwrap(), Snapshot::Failed(why) => panic!("{name}: the fill failed: {why:?}"), _ => tokio::time::sleep(Duration::from_millis(20)).await }
    }}).await.unwrap_or_else(|_| panic!("{name}: the figures never became ready"));
    let (locale, prefix) = if name == "fx_de" { ("de", "/de") } else { ("en", "") };
    let clock = app.clock.clone();
    let actual = app.db(move |c| loading::render(c, user, &snapshot, locale, "token", prefix, &*clock)).await.unwrap().unwrap_or_else(|| panic!("{name}: nothing kept for the account's own key"));
    // The panels Rails renders in the user's locale only where a broadcast does; the fragments are in English.
    let parts: &[&str] = if name == "fx_de" { &["tile"] } else { &["tile", "metrics", "chart"] };
    let ours = actual["bots"].as_object().unwrap();
    assert!(!ours.is_empty(), "{name}: no bots");
    for (id, figures) in ours {
        for part in parts {
            let rails = expected["bots"][id][*part].as_str().unwrap_or_else(|| panic!("{name}: Rails has no {part} for bot {id}"));
            assert_eq!(common::html::normalize(figures[*part].as_str().unwrap()), common::html::normalize(rails), "{name} bot {id} {part}");
        }
    }
    assert_eq!(common::html::normalize(actual["account"].as_str().unwrap()), common::html::normalize(expected["account"].as_str().unwrap()), "{name} account");
    let c = rusqlite::Connection::open_with_flags(dir.join("production.sqlite3"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    // The cable bytes of every tile and of the headline: what Rails' broadcast_pnl_update and broadcast_global_pnl_update sent.
    for (stream, payload) in deltabadger::web::figure::streams(&c, user, &actual).unwrap() {
        let target = target(&payload);
        if !(target == "global-pnl" || target.starts_with("pnl_bots_")) { continue; }
        let wanted = expected["broadcasts"].as_array().unwrap().iter().find(|entry| entry[0] == stream && entry[1].as_str().is_some_and(|s| s.contains(&format!("target=\"{target}\""))))
            .unwrap_or_else(|| panic!("{name}: no Rails broadcast to {stream} {target}"));
        assert_eq!(common::html::normalize(&payload), common::html::normalize(wanted[1].as_str().unwrap()), "{name} broadcast {stream} {target}");
    }
    let headline = actual["account"].as_str().unwrap();
    let tiles: String = ours.values().map(|bot| bot["tile"].as_str().unwrap()).collect();
    match name {
        "fx_chf" => assert!(headline.contains("<small>Fr.</small>") && headline.contains("data-pnl-spark-suffixed-value=\"true\""), "{name}: {headline}"),
        "fx_de" => assert!(headline.contains("data-pnl-spark-delimiter-value=\",\""), "{name}: {headline}"),
        // The USD, USDT and archived baskets have traded; the empty one has no amount.
        "fx_usdt_usd" => assert_eq!(tiles.matches("pnl-amount").count(), 3, "{name}: every traded bot carries its amount: {tiles}"),
        "fx_hidden_eur" => assert!(!tiles.contains("pnl-amount") && !headline.contains("pnl-amount") && !headline.contains("profit-value"), "{name}"),
        "fx_rates_down" => assert!(!actual.to_string().contains("Currency conversion unavailable"), "{name}: amounts are absent, never the page"),
        "fx_display_down" => assert!(headline.contains("data-pnl-spark-rate-value=\"1.0\"") && headline.contains("<small>$</small>"), "{name}: {headline}"),
        _ => {}
    }
}

/// The review's conflicting reports: SQLite accepts both, serde_json cannot decode the second's metadata.
/// Exercise both clocks through the core, pages, and the actual socket, using an isolated copy of the Rails fixture.
async fn unreadable_split_report(dir: &Path, sc: &Value, expected: &Value, now: At, name: &str) {
    use deltabadger::{figures::{db, walk, FiguresError}, web::{App, Config, figure::loading::{self, Source}, session::{self, SessionData}}};
    let copy = tempfile::tempdir().unwrap();
    let database = copy.path().join("production.sqlite3");
    std::fs::copy(dir.join("production.sqlite3"), &database).unwrap();
    let c = rusqlite::Connection::open(&database).unwrap();
    let user = sc["user_id"].as_i64().unwrap();
    let bot = sc["bot_ids"][0].as_i64().unwrap();
    c.execute("UPDATE bots SET status=2", []).unwrap(); // stopped, with shares already held
    assert_eq!(c.execute("UPDATE account_transactions SET raw_data=?1 WHERE entry_type=15", [r#"{"corporate_action":"split","split_ratio":"2:1"}"#]).unwrap(), 1);
    let raw = format!(r#"{{"corporate_action":"split","split_ratio":"3:1","metadata":{}}}"#, "9".repeat(400));
    assert!(serde_json::from_str::<Value>(&raw).is_err());
    assert_eq!(c.query_row("SELECT json_valid(?1)", [&raw], |r| r.get::<_, i64>(0)).unwrap(), 1);
    c.execute("INSERT INTO account_transactions (user_id,exchange_id,entry_type,base_currency,base_amount,transacted_at,created_at,updated_at,raw_data) SELECT user_id,exchange_id,entry_type,base_currency,base_amount,transacted_at,created_at,updated_at,?1 FROM account_transactions WHERE entry_type=15", [&raw]).unwrap();
    assert_eq!(c.query_row("SELECT count(*) FROM account_transactions WHERE entry_type=15", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
    let subject = db::Subject::load(&c, bot).unwrap();
    let split_at: String = c.query_row("SELECT min(transacted_at) FROM account_transactions WHERE entry_type=15", [], |r| r.get(0)).unwrap();
    let expires = db_time(&split_at) + chrono::Duration::seconds(deltabadger::figures::splits::QUARANTINE_SECONDS);
    assert_eq!(now.utc() < expires, name == "split_fresh", "both sides of quarantine must be exercised");
    assert!(matches!(db::split_rows(&c, user, &[subject.bot.exchange_id.unwrap()]), Err(FiguresError::Data(_))), "unreadable split report must fail the whole read");
    assert!(matches!(walk::metrics(&c, &subject, now), Err(FiguresError::Data(_))), "a partial split read must not produce a share count");
    let cache = Cache::default();
    let reader = Reader::new(&cache, now.utc().timestamp());
    assert!(matches!(deltabadger::web::figure::account(&c, user, &reader, now, "en", "", ""), Err(FiguresError::Data(_))));
    let password: String = c.query_row("SELECT encrypted_password FROM users WHERE id=?1", [user], |r| r.get(0)).unwrap();
    let env = common::web::env(common::web::SECRET);
    let app = App::new(Config::from_env(&env).unwrap(), &env, c, common::web::TestClock::at(sc["at"].as_str().unwrap())).unwrap()
        .with_figure_source(Source::Script(sc["script"].clone())).unwrap();
    let signed = SessionData { user: Some((user, password.chars().take(29).collect())), ..SessionData::default() };
    let cookie = session::seal(&app.keys.session, &signed, now.utc());
    let wanted: BTreeSet<(String, String)> = expected["broadcasts"].as_array().unwrap().iter()
        .map(|entry| (entry[0].as_str().unwrap().to_string(), target(entry[1].as_str().unwrap()))).collect();
    assert_eq!(wanted.len(), 4);
    let streams = wanted.iter().map(|(stream, _)| stream.clone()).collect();
    let mut cable = Cable::open(&app, &cookie, &streams).await;
    loading::publish(&app, user).await.unwrap();
    let unavailable = |html: &str| {
        assert!(html.contains("no-value"), "{name}: an unreadable split produced a money figure: {html}");
        assert!(!html.contains("data-symbol="), "{name}: a partial share count escaped");
        assert!(!html.contains("data-bot--chart-series"), "{name}: a partial chart escaped");
    };
    let mut got = BTreeSet::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !wanted.iter().all(|key| got.contains(key)) {
            let value = cable.next().await;
            let (Some(identifier), Some(html)) = (value["identifier"].as_str(), value["message"].as_str()) else { continue };
            unavailable(html); // every payload, including one a later replacement might hide
            got.insert((cable.streams[identifier].clone(), target(html)));
        }
    }).await.expect("unavailable publication must replace every target");
    assert_eq!(got, wanted, "{name}: unexpected publication target");
    // Check any additional publications too, not merely the last payload per target.
    app.hub.broadcast(&format!("user_{user}:bot_updates"), "split-checked");
    loop {
        let value = cable.next().await;
        if value["message"] == "split-checked" { break; }
        if let Some(html) = value["message"].as_str() { unavailable(html); }
    }
    let mut browser = common::web::Browser { cookie: Some(cookie), page: None };
    let page = browser.get(&app, &format!("/bots/{bot}")).await;
    assert_eq!(page.status, 200);
    let doc = scraper::Html::parse_document(&page.body);
    unavailable(&doc.select(&scraper::Selector::parse("#metrics").unwrap()).next().unwrap().html());
    let chart = browser.get(&app, &format!("/bots/{bot}/chart")).await;
    assert_eq!(chart.status, 200);
    unavailable(&chart.body);
}

fn db_time(text: &str) -> chrono::DateTime<chrono::Utc> {
    At::from_sql(text).unwrap().utc()
}

/// Rails over time (script/rust/page_figures.rb `chronology`), on the owner's account: warm, a reread with no write, an
/// order filled, a reread, the cut, and the provider failing at the next cut. At each step what this build shows (its
/// kept figures, or the ones its fill published) equals Rails' figures computed fresh at the moment the shown figures
/// were computed (`t_c`, the chart's last label), never relabelled to the moment of the read (`t_r`). Rails' own
/// cached read (`actual`) differs from fresh only inside W1 (`Bot::UpdateMetricsJob` refreshes the prices and keeps the
/// chart it cached until the cut) and agrees again at the cut.
#[tokio::test]
async fn chronology_matches_rails_fresh_bytes() {
    use deltabadger::web::{App, Config, figure::loading::{self, Source, Snapshot}};
    let scratch = tempfile::tempdir().unwrap();
    record(&scratch, "chronology");
    let dir = scratch.path().join("owner_account");
    let sc: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("scenario.json")).unwrap()).unwrap();
    let chronology: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("chronology.json")).unwrap()).unwrap();
    let work = tempfile::tempdir().unwrap();
    let file = work.path().join("production.sqlite3");
    std::fs::copy(dir.join("initial.sqlite3"), &file).unwrap();
    let user = sc["user_id"].as_i64().unwrap();
    let env = common::web::env(common::web::SECRET);
    let steps = chronology["steps"].as_array().unwrap();
    let clock = common::web::TestClock::at(steps[0]["t_r"].as_str().unwrap());
    let open = |script: &Value| App::new(Config::from_env(&env).unwrap(), &env, rusqlite::Connection::open(&file).unwrap(), clock.clone()).unwrap().with_figure_source(Source::Script(script.clone())).unwrap();
    let mut app = open(&sc["script"]);
    let w1: Vec<&str> = chronology["w1"].as_array().unwrap().iter().map(|n| n.as_str().unwrap()).collect();
    let parts = |figures: &Value| -> BTreeMap<String, Vec<String>> {
        let mut out = BTreeMap::new();
        for (id, bot) in figures["bots"].as_object().unwrap() {
            for part in ["tile", "metrics", "chart"] { out.insert(format!("{id} {part}"), common::html::normalize(bot[part].as_str().unwrap())); }
        }
        out.insert("account".into(), common::html::normalize(figures["account"].as_str().unwrap()));
        out
    };
    for step in steps {
        let name = step["name"].as_str().unwrap();
        // The provider's answers change from this step on: a process with the new market, and nothing kept from the old one.
        if let Some(script) = step.get("script") { app = open(script); }
        clock.set(common::web::at(step["t_r"].as_str().unwrap()));
        let writer = rusqlite::Connection::open(&file).unwrap();
        for write in step["writes"].as_array().into_iter().flatten() {
            let (table, row) = (write[0].as_str().unwrap(), write[1].as_object().unwrap());
            let columns: Vec<&String> = row.keys().collect();
            let sql = format!("INSERT INTO {table}({}) VALUES ({})", columns.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(","), vec!["?"; columns.len()].join(","));
            let values: Vec<rusqlite::types::Value> = row.values().map(|v| match v {
                Value::Null => rusqlite::types::Value::Null,
                Value::Number(n) if n.is_i64() => rusqlite::types::Value::Integer(n.as_i64().unwrap()),
                Value::Number(n) => rusqlite::types::Value::Real(n.as_f64().unwrap()),
                Value::String(s) => rusqlite::types::Value::Text(s.clone()),
                other => rusqlite::types::Value::Text(other.to_string()),
            }).collect();
            writer.execute(&sql, rusqlite::params_from_iter(values)).unwrap();
        }
        drop(writer);
        let reread = name.starts_with("reread");
        let mut snapshot = loading::prepare(&app, user).await.unwrap();
        if reread { assert!(matches!(snapshot, Snapshot::Ready(..)), "{name}: a reread with no write is served what was kept, with no fill"); }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while matches!(snapshot, Snapshot::Cold) {
            assert!(std::time::Instant::now() < deadline, "{name}: the fill never ended");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            snapshot = loading::prepare(&app, user).await.unwrap();
        }
        let t_c = common::web::at(step["t_c"].as_str().unwrap());
        if let Snapshot::Ready(_, at, ..) = &snapshot { assert_eq!(at.utc(), t_c, "{name}: the figures shown were computed at t_c, whenever they are read"); }
        let shown = snapshot.clone();
        let read_at = deltabadger::engine::FixedClock(common::web::at(step["t_r"].as_str().unwrap()));
        let rendered = app.db(move |c| loading::render(c, user, &shown, "en", "", "", &read_at)).await.unwrap().expect("figures");
        let (ours, fresh, actual) = (parts(&rendered), parts(&step["fresh"]), parts(&step["actual"]));
        if matches!(snapshot, Snapshot::Failed(_)) {
            // Nothing was kept: the market refused this fill. What was kept before it is never shown past its cut; the
            // page shows this build's unavailable state (Rails shows its stale figures there, a difference older than kept
            // figures, held by the grid's refused-price case).
            assert_eq!(name, "provider_failure", "{name}: the fill failed");
            assert!(ours.values().all(|lines| lines.iter().any(|line| line.contains("\"—\""))), "{name}: {ours:?}");
        } else {
            for (key, want) in &fresh { assert_eq!(ours.get(key), Some(want), "{name}: {key}: this build against Rails computed fresh at t_c"); }
            assert_eq!(ours.len(), fresh.len(), "{name}");
        }
        let differing: Vec<&String> = fresh.keys().filter(|key| actual[*key] != fresh[*key]).collect();
        if w1.contains(&name) { assert!(!differing.is_empty() && differing.iter().all(|key| key.ends_with(" chart") || *key == "account"), "{name}: W1 is the chart Rails kept: {differing:?}"); }
        else { assert!(differing.is_empty(), "{name}: Rails' cached read and its fresh computation differ outside W1: {differing:?}"); }
    }
}

/// A page whose figures are not kept (here: ready figures kept in the account's own locale, the page read in another)
/// shows Rails' loading state at every figure target, exactly as Rails' cold page shows it, and computes nothing.
#[tokio::test]
async fn cold_pages_match_rails_loading_state() {
    use deltabadger::web::{App, Config, session::{self, SessionData}, figure::loading::{self, Source}};
    let scratch = tempfile::tempdir().unwrap();
    record(&scratch, "figures");
    let element = |body: &str, css: &str| -> Option<Vec<String>> {
        let doc = scraper::Html::parse_document(body);
        doc.select(&scraper::Selector::parse(css).unwrap()).next().map(|e| common::html::normalize(&e.html()))
    };
    for name in ["basket_buys", "hidden", "owner_account"] {
        let dir = scratch.path().join(name);
        let sc: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("scenario.json")).unwrap()).unwrap();
        let expected: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("rails.json")).unwrap()).unwrap();
        let user = sc["user_id"].as_i64().unwrap();
        let work = tempfile::tempdir().unwrap();
        let file = work.path().join("production.sqlite3");
        std::fs::copy(dir.join("production.sqlite3"), &file).unwrap();
        // The account reads its pages in French: what a fill keeps is French.
        rusqlite::Connection::open(&file).unwrap().execute("UPDATE users SET locale='fr' WHERE id=?1", [user]).unwrap();
        let env = common::web::env(common::web::SECRET);
        let app = App::new(Config::from_env(&env).unwrap(), &env, rusqlite::Connection::open(&file).unwrap(), common::web::TestClock::at(sc["at"].as_str().unwrap())).unwrap()
            .with_figure_source(Source::Script(sc["script"].clone())).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !matches!(loading::prepare(&app, user).await.unwrap(), loading::Snapshot::Ready(..)) { tokio::time::sleep(std::time::Duration::from_millis(20)).await; }
        }).await.unwrap();
        let password: String = rusqlite::Connection::open(&file).unwrap().query_row("SELECT encrypted_password FROM users WHERE id=?1", [user], |r| r.get(0)).unwrap();
        let signed = SessionData { user: Some((user, password.chars().take(29).collect())), ..SessionData::default() };
        let mut browser = common::web::Browser { cookie: Some(session::seal(&app.keys.session, &signed, common::web::at(sc["at"].as_str().unwrap()))), page: None };
        for id in sc["bot_ids"].as_array().unwrap().iter().map(|id| id.to_string()) {
            let page = browser.get(&app, &format!("/en/bots/{id}")).await;
            assert_eq!(page.status, 200, "{name}: {}", page.body);
            let rails = expected["cold"]["bots"][&id]["page"].as_str().unwrap();
            for css in ["#metrics", "turbo-frame#bot_chart"] {
                assert_eq!(element(&page.body, css).map(|lines| lines.into_iter().map(|l| l.replace("/en/bots", "/bots")).collect::<Vec<_>>()), element(rails, css), "{name} bot {id}: {css}");
            }
        }
        if let Some(rails) = expected["cold"]["dashboard"].as_str() {
            let page = browser.get(&app, "/en/bots").await;
            assert_eq!(page.status, 200, "{name}");
            assert_eq!(element(&page.body, "#global-pnl"), element(rails, "#global-pnl"), "{name}: the headline");
            for id in sc["bot_ids"].as_array().unwrap() {
                for kind in ["dca_multi_asset", "dca_index"] {
                    let css = format!("#pnl_bots_{kind}_{id}");
                    assert_eq!(element(&page.body, &css), element(rails, &css), "{name}: {css}");
                }
            }
        }
    }
}
