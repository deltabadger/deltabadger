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
#[tokio::test]
async fn figures_fragments_match_the_rails_page_partials() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let out = std::process::Command::new(root.join("bin/rails")).current_dir(root)
        .args(["runner", "script/rust/pages.rb", "figures", scratch.path().to_str().unwrap()])
        .env("SECRET_KEY_BASE",common::web::SECRET).env("APP_ROOT_URL", "http://localhost:3000").env("SKIP_TEST_DATABASE", "true").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    for entry in std::fs::read_dir(scratch.path()).unwrap() {
        let dir = entry.unwrap().path();
        let name = dir.file_name().unwrap().to_str().unwrap();
        let sc: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("scenario.json")).unwrap()).unwrap();
        let expected: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("rails.json")).unwrap()).unwrap();
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
            use deltabadger::web::figure::loading::{self,Snapshot};
            let rendered=loading::render(&c,sc["user_id"].as_i64().unwrap(),&Snapshot::Failed(None),"en","token","").unwrap().unwrap();
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
            // What Rails broadcast for every figure of the account, by (stream, target), and the streams a page subscribes.
            let rails:BTreeMap<(String,String),String>=expected["broadcasts"].as_array().unwrap().iter()
                .map(|entry|((entry[0].as_str().unwrap().to_string(),target(entry[1].as_str().unwrap())),entry[1].as_str().unwrap().to_string())).collect();
            let wanted:BTreeSet<(String,String)>=rails.keys().cloned().collect();
            let streams:BTreeSet<String>=wanted.iter().map(|(stream,_)|stream.clone()).collect();
            let user_stream=format!("user_{user}:bot_updates");
            // Cold to warm: the page's streams are connected before any figure is known, the page asks while the fill is in
            // flight (its first market answer is held back), and the end of the fill replaces every spinner.
            let mut cable=Cable::open(&app,&cookie,&streams).await;
            assert!(matches!(loading::prepare(&app,user).await.unwrap(),loading::Snapshot::Cold));
            loading::publish(&app,user).await.unwrap();
            delivered(name,"the end of the fill",&cable.until(&wanted).await,&rails);
            tokio::time::timeout(std::time::Duration::from_secs(5),async {
                loop {
                    match loading::prepare(&app,user).await.unwrap() {
                        loading::Snapshot::Ready(_,_,_)=>break,
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
            let chart=browser.get(&app,&format!("/bots/{id}/chart")).await;
            assert_eq!(chart.status,200);
            assert!(chart.body.contains("data-controller=\"bot--chart\""));
            }
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
                let loading::Snapshot::Ready(mut refused,at,revision)=prepared.clone() else { panic!("not warm") };
                // A missing cache entry must never expose the core's successful ledger fallback.
                let pending=loading::Snapshot::Ready(Cache::default(),at,revision);
                let pending=app.db(move|c| loading::render(c,user,&pending,"en","token","")).await.unwrap().unwrap();
                let failed=app.db(move|c| loading::render(c,user,&loading::Snapshot::Failed(None),"en","token","")).await.unwrap().unwrap();
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
                let refused=loading::Snapshot::Ready(refused,at,revision);
                let refused=app.db(move|c| loading::render(c,user,&refused,"en","token","")).await.unwrap().unwrap();
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
                    let snapshot = loading::Snapshot::Ready(cache,at,revision);
                    let rendered = app.db(move|c| loading::render(c,user,&snapshot,"en","token","")).await.unwrap().unwrap();
                    assert_eq!(rendered,failed,"failed candles published a chart or account sparkline");
                }
                // Index tickers use the venue's entire symbol union. Synchronization inserts
                // another listing after prepare; all dependent targets must lose their numbers.
                app.db(|c| {
                    c.execute("INSERT INTO tickers (base,base_asset_id,base_decimals,created_at,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,updated_at) SELECT 'USD_TEST',quote_asset_id,base_decimals,created_at,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,'USD_TEST',updated_at FROM tickers WHERE ticker='AAA' LIMIT 1",[])?;
                    Ok(())
                }).await.unwrap();
                let changed=app.db(move|c| {
                    assert!(loading::symbols(c,user).unwrap().iter().any(|s|s=="USD_TEST"));
                    loading::render(c,user,&prepared,"en","token","")
                }).await.unwrap().unwrap();
                assert_eq!(changed,failed,"new ticker published a ledger figure or account total");
                app.db(|c| { c.execute("DELETE FROM tickers WHERE ticker='USD_TEST'",[])?; Ok(()) }).await.unwrap();
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
                    while !matches!(loading::prepare(&mounted_app,user).await.unwrap(),loading::Snapshot::Ready(_,_,_)) { tokio::time::sleep(Duration::from_millis(20)).await; }
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
                    assert!(loading::render(c,user,&loading::Snapshot::Failed(None),"en","","").is_err(),
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
                            loading::Snapshot::Ready(_,_,_)=>break,
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
                if ["stranded_offset", "liquidations"].contains(&name) && part == "metrics" {
                    let refusal = "<p role=\"status\">Redeploy unavailable</p>\n";
                    assert!(got.contains(refusal));
                    assert_eq!(expected["offset"][0], expected["offset"][1], "{name}: a Rails read keeps the saved decline");
                    got = got.replacen(refusal, "", 1);
                }
                assert_eq!(common::html::normalize(&got), common::html::normalize(parts[part].as_str().unwrap()), "{name} bot {id} {part}");
            }
        }
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
