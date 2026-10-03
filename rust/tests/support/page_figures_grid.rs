use super::common;
use deltabadger::figures::{at::At, page_market::{Cache, Reader}};
use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use serde_json::Value;
use std::cell::RefCell;
use std::path::Path;

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
        if std::env::var("FIGURE_ROUTES").as_deref() != Ok("0") && ["basket_buys", "hidden", "owner_account"].contains(&name) {
            use deltabadger::web::{App,Config,session::{self,SessionData},figure::loading::{self,Source}};
            let env=common::web::env(common::web::SECRET);
            let clock=common::web::TestClock::at(sc["at"].as_str().unwrap());
            let app=App::new(Config::from_env(&env).unwrap(),&env,rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap(),clock).unwrap().with_figure_source(Source::Script(sc["script"].clone())).unwrap();
            let user=sc["user_id"].as_i64().unwrap();
            assert!(matches!(loading::prepare(&app,user).await.unwrap(),loading::Snapshot::Cold));
            tokio::time::timeout(std::time::Duration::from_secs(5),async {
                loop {
                    match loading::prepare(&app,user).await.unwrap() {
                        loading::Snapshot::Ready(_,_,_)=>break,
                        loading::Snapshot::Failed=>panic!("fill failed"),
                        _=>tokio::task::yield_now().await,
                    }
                }
            }).await.unwrap();
            let password:String=c.query_row("SELECT encrypted_password FROM users WHERE id=?1",[user],|r|r.get(0)).unwrap();
            let signed=SessionData { user:Some((user,password.chars().take(29).collect())),..SessionData::default() };
            let mut browser=common::web::Browser { cookie:Some(session::seal(&app.keys.session,&signed,now.utc())),page:None };
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
                let pending=app.db(move|c| Ok(loading::render(c,user,&pending,"en","token",""))).await.unwrap().unwrap();
                let failed=app.db(move|c| Ok(loading::render(c,user,&loading::Snapshot::Failed,"en","token",""))).await.unwrap().unwrap();
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
                let refused=app.db(move|c| Ok(loading::render(c,user,&refused,"en","token",""))).await.unwrap().unwrap();
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
                    let rendered = app.db(move|c| Ok(loading::render(c,user,&snapshot,"en","token",""))).await.unwrap().unwrap();
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
                    Ok(loading::render(c,user,&prepared,"en","token",""))
                }).await.unwrap().unwrap();
                assert_eq!(changed,failed,"new ticker published a ledger figure or account total");
                app.db(|c| { c.execute("DELETE FROM tickers WHERE ticker='USD_TEST'",[])?; Ok(()) }).await.unwrap();
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
                            loading::Snapshot::Failed=>panic!("corrected fill failed"),
                            _=>tokio::task::yield_now().await,
                        }
                    }
                }).await.unwrap();
                app.db(move|c| { c.execute("UPDATE transactions SET quote_amount_exec=?1 WHERE id=(SELECT min(id) FROM transactions)",[original])?; Ok(()) }).await.unwrap();
            }

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
                    assert_ne!(expected["offset"][0], expected["offset"][1]);
                    got = got.replacen(refusal, "", 1);
                }
                assert_eq!(common::html::normalize(&got), common::html::normalize(parts[part].as_str().unwrap()), "{name} bot {id} {part}");
            }
        }
    }
}
