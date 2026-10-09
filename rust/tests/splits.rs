mod common;
use chrono::{DateTime, SecondsFormat, Utc};
use common::seed::{self, BotSpec};
use deltabadger::engine::{basket, model, splits};
use rusqlite::{params, Connection};
use serde_json::Value;
use std::collections::BTreeMap;

const T: &str = "2026-01-01 00:00:00";

/// The recorded case's rows, inserted as the recorder inserted them; returns the bot and each base's asset id.
fn seed_case(c: &Connection, s: &seed::Seeded, case: &Value) -> (i64, BTreeMap<String, i64>) {
    c.execute("INSERT INTO users (email, encrypted_password, name, admin, created_at, updated_at) VALUES ('stranger@example.com', 'x', 'Stranger', 0, ?1, ?1)", [T]).unwrap();
    let stranger = c.last_insert_rowid();
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Kraken', 'Kraken', '0.25', '0.4', ?1, ?1)", [T]).unwrap();
    let kraken = c.last_insert_rowid();
    let mut assets = BTreeMap::new();
    for a in case["assets"].as_array().unwrap() {
        let (base, symbol) = (a["base"].as_str().unwrap(), a["symbol"].as_str().unwrap());
        c.execute("INSERT INTO assets (external_id, symbol, name, category, instrument_type, created_at, updated_at) VALUES (?1, ?2, ?2, 'Stock', 'stock', ?3, ?3)",
                  params![format!("{base}.W"), symbol, T]).unwrap();
        let asset = c.last_insert_rowid();
        c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
                   minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) \
                   VALUES (?1, ?2, ?2, 'USD', ?3, ?4, 9, 2, 2, '0.000000001', '1', 1, 1, ?5, ?6)",
                  params![s.exchange_id, base, asset, s.quote, T, seed::SYNCED]).unwrap();
        assets.insert(base.to_string(), asset);
    }
    let members = case["allocations"].as_array().unwrap();
    let weights: Vec<(i64, f64)> = members.iter().map(|b| (assets[b.as_str().unwrap()], 1.0 / members.len() as f64)).collect();
    let bot = seed::insert_bot(c, s, &BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&weights));
    for b in case["buys"].as_array().unwrap() {
        let f = |i: usize| b[i].as_str().unwrap().to_string();
        let (base, at, amount, price) = (f(0), f(1), f(2), f(3));
        let symbol: String = c.query_row("SELECT symbol FROM assets WHERE id = ?1", [assets[&base]], |r| r.get(0)).unwrap();
        let quote = (&deltabadger::ruby::BigDec::parse(&amount).unwrap() * &deltabadger::ruby::BigDec::parse(&price).unwrap()).to_s_f();
        c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, amount, quote_amount, price, \
                   amount_exec, quote_amount_exec, base, quote, base_asset_id, quote_asset_id, bot_interval, bot_quote_amount, transaction_type, \
                   error_messages, created_at, updated_at) \
                   VALUES (?1, ?2, ?3, 0, 2, 0, 0, ?4, ?5, ?6, ?4, ?5, ?7, 'USD', ?8, ?9, 'week', 60, 'REGULAR', '[]', ?10, ?10)",
                  params![bot, s.exchange_id, format!("W-{base}-{at}"), amount, quote, price, symbol, assets[&base], s.quote, at]).unwrap();
    }
    for sp in case["splits"].as_array().unwrap() {
        let user = if sp["user"] == "other" { stranger } else { s.user_id };
        let exchange = if sp["venue"] == "kraken" { kraken } else { s.exchange_id };
        let mut raw = serde_json::json!({ "activity_type": "SPLIT", "symbol": sp["name"], "corporate_action": "split" });
        if let Some(r) = sp["ratio"].as_str() { raw["split_ratio"] = r.into(); }
        c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, transacted_at, raw_data, created_at, updated_at) \
                   VALUES (?1, ?2, 15, ?3, '0', ?4, ?5, ?4, ?4)", params![user, exchange, sp["name"].as_str().unwrap(), sp["at"].as_str().unwrap(), raw.to_string()]).unwrap();
    }
    (bot, assets)
}

#[test]
fn every_recorded_rails_split_walk_is_reproduced() {
    let cases = common::vectors()["split_walks"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 19);
    for case in cases {
        let (_d, o, s) = common::install_alpaca();
        let (id, assets) = seed_case(&o.primary, &s, &case);
        let bot = model::load_bot(&o.primary, id).unwrap();
        let now: DateTime<Utc> = case["now"].as_str().unwrap().parse().unwrap();
        let walk = basket::walk(&o.primary, &bot, now).unwrap();
        let base_of: BTreeMap<i64, &String> = assets.iter().map(|(b, a)| (*a, b)).collect();
        let amounts: BTreeMap<String, String> = walk.amounts.iter().map(|(a, v)| (base_of[a].to_string(), v.to_s_f())).collect();
        let want = &case["expected"];
        let want_amounts: BTreeMap<String, String> = want["amounts"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string())).collect();
        assert_eq!(amounts, want_amounts, "{}", case["name"]);
        let restated = walk.restated_at_us.map(|us| DateTime::from_timestamp_micros(us).unwrap().to_rfc3339_opts(SecondsFormat::Micros, true));
        assert_eq!(restated.as_deref(), want["restated_at"].as_str(), "{}", case["name"]);
        assert_eq!(splits::untrusted(&o.primary, &bot, now).unwrap(), want["untrusted"].as_bool().unwrap(), "{}", case["name"]);
    }
}


#[test]
fn plausible_but_wrong_split_ratios_fail_the_position_backstop() {
    use deltabadger::ruby::BigDec;
    let d = |s| BigDec::parse(s).unwrap();
    // S3/S5 and S8 pass the row rules. S7b is only slightly wrong and must not use a relative tolerance.
    for (ratio, delta, first, before, actual) in [
        ("3:2", "5", "-10", "10", "30"),
        ("10:1", "90", "-10", "10", "200"),
        ("182:181", "5", "-1000", "1000", "1005"),
    ] {
        let row = serde_json::json!({"base_amount":delta,"raw_data":{"qty":first,"split_ratio":ratio,"merged_activity_ids":["a","b"]}});
        assert_eq!(splits::row_verdict(&[row]), "trusted");
        let (p,q) = ratio.split_once(':').unwrap();
        let expected = &d(before) * &d(p).div(&d(q)).unwrap();
        assert!(!splits::position_agrees(&expected,&d(actual)), "{ratio}");
    }
    assert!(splits::position_agrees(&d("10"),&d("10.000000001")));
    assert!(!splits::position_agrees(&d("10"),&d("10.000000002")));
}

#[tokio::test(flavor="current_thread")]
async fn a_wrong_split_stands_down_then_correct_rows_clear_without_any_order() {
    use deltabadger::{venue::{alpaca::{AlpacaVenue,Urls},http::ScriptedTransport}, ruby::BigDec};
    use serde_json::json;
    let (_dir,o,s)=common::install_alpaca();
    let (asset,_)=seed::add_alpaca_stock(&o.primary,&s,"KLAC");
    let id=seed::insert_bot(&o.primary,&s,&BotSpec::weekly(60.0,"2026-09-01 14:00:00").weights(&[(asset,1.0)]));
    let tx=common::seed::TxSpec {status:0,external_status:Some(2),external_id:Some("old-fill".into()),order_type:0,amount:Some("10"),quote_amount:Some("1000"),price:Some("100"),amount_exec:Some("10"),quote_amount_exec:Some("1000"),created_at:"2026-09-01 14:00:01".into()};
    seed::insert_stock_tx(&o.primary,&s,id,asset,"KLAC",&tx);
    let row=seed::insert_split(&o.primary,&s,"KLAC","2026-09-05 00:00:00",Some("3:2"));
    o.primary.execute("UPDATE account_transactions SET base_amount=5,raw_data=?1 WHERE id=?2",params![json!({"corporate_action":"split","qty":"-10","split_ratio":"3:2","merged_activity_ids":["a","b"]}).to_string(),row]).unwrap();
    let transport=ScriptedTransport::from_script(&json!({"GET /v2/positions":[{"status":200,"body":[{"symbol":"KLAC","asset_class":"us_equity","qty":"30"}]}]}));
    let venue=AlpacaVenue::new(transport.clone(),Urls::for_passphrase(Some("paper")));
    let bot=model::load_bot(&o.primary,id).unwrap();
    let now="2026-09-10T14:00:00Z".parse().unwrap();
    assert!(splits::refusal(&o.primary,&venue,&bot,now).await.unwrap().unwrap().contains("venue quantity 30"));
    // Simulate the corrected row written by a later Rails grouping fix. The engine never rewrites it itself.
    o.primary.execute("UPDATE account_transactions SET base_amount=20,raw_data=json_set(raw_data,'$.split_ratio','3:1') WHERE id=?1",[row]).unwrap();
    assert!(splits::refusal(&o.primary,&venue,&bot,now).await.unwrap().is_none());
    assert_eq!(basket::walk(&o.primary,&bot,now).unwrap().amounts[&asset],BigDec::from_i64(30));
    assert!(transport.posted_orders().is_empty(),"reconciliation itself never trades");
    let second=seed::insert_bot(&o.primary,&s,&BotSpec::weekly(60.0,"2026-09-01 14:00:00").weights(&[(asset,1.0)]));
    seed::insert_stock_tx(&o.primary,&s,second,asset,"KLAC",&common::seed::TxSpec {external_id:Some("second-fill".into()),..tx});
    assert!(splits::refusal(&o.primary,&venue,&bot,now).await.unwrap().is_some(),"one bot is not the account");
    let both=ScriptedTransport::from_script(&json!({"GET /v2/positions":[{"status":200,"body":[{"symbol":"KLAC","asset_class":"us_equity","qty":"60"}]}]}));
    let both=AlpacaVenue::new(both,Urls::for_passphrase(Some("paper")));
    assert!(splits::refusal(&o.primary,&both,&bot,now).await.unwrap().is_none(),"sum both bots' restated holdings once");
    let unreadable=ScriptedTransport::from_script(&json!({"GET /v2/positions":[{"status":200,"body":[{"symbol":"KLAC","asset_class":"us_equity"}]}]}));
    let unreadable=AlpacaVenue::new(unreadable,Urls::for_passphrase(Some("paper")));
    assert!(splits::refusal(&o.primary,&unreadable,&bot,now).await.unwrap().is_some(),"missing quantity never means zero");

}


#[test]
fn long_split_walks_refuse_both_numeric_growth_and_cumulative_work() {
    use deltabadger::{ruby::BigDec,figures::budget};
    let (_d,o,s)=common::install_alpaca();
    let case=serde_json::json!({"assets":[{"base":"AAA","symbol":"AAA"}],"allocations":["AAA"],"buys":[["AAA","2026-01-01 00:00:00","1","100"]],"splits":[]});
    let (id,assets)=seed_case(&o.primary,&s,&case);
    let bot=model::load_bot(&o.primary,id).unwrap();
    let at="2026-01-02T00:00:00Z".parse::<DateTime<Utc>>().unwrap().timestamp_micros();
    let events:Vec<_>=(0..2000).map(|i|splits::SplitEvent{at_us:at+i,asset_id:assets["AAA"],factor:BigDec::parse("1.5").unwrap()}).collect();
    let err=basket::walk_with(&o.primary,&bot,&events).unwrap_err();
    assert!(format!("{err:?}").contains("split walk"));
    let events:Vec<_>=(0..2000).map(|i|splits::SplitEvent{at_us:at+i,asset_id:assets["AAA"],factor:BigDec::parse(if i%2==0 {"2"} else {"0.5"}).unwrap()}).collect();
    let (result,used)=budget::scope(budget::Limits{steps:1000,held:1000},||basket::walk_with(&o.primary,&bot,&events));
    assert!(format!("{:?}",result.unwrap_err()).contains("split walk"));
    assert!((900..=1000).contains(&used.steps),"cumulative split charges, before work: {used:?}");
    assert!(budget::scope(budget::Limits{steps:1000,held:1000},||basket::walk_with(&o.primary,&bot,&events[..20])).0.is_ok());
}

#[test]
fn an_archived_account_history_exhausts_one_budget_and_visibly_stands_down() {
    use deltabadger::{figures::budget, engine::{tick, FixedClock}, venue::{alpaca::{AlpacaVenue, Urls}, http::ScriptedTransport}};
    use futures_util::FutureExt;
    use serde_json::json;
    let (_d, o, s) = common::install_alpaca();
    let (asset, _) = seed::add_alpaca_stock(&o.primary, &s, "AAPL");
    let spec = BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&[(asset,1.0)]);
    let id = seed::insert_bot(&o.primary, &s, &spec);
    let archived = seed::insert_bot(&o.primary, &s, &spec);
    o.primary.execute("UPDATE bots SET status=7 WHERE id=?1", [archived]).unwrap();
    let mut tx = seed::TxSpec {status:0,external_status:Some(2),external_id:Some("old-fill".into()),order_type:0,amount:Some("10"),quote_amount:Some("1000"),price:Some("100"),amount_exec:Some("10"),quote_amount_exec:Some("1000"),created_at:"2026-09-01 14:00:01".into()};
    seed::insert_stock_tx(&o.primary,&s,id,asset,"AAPL",&tx);
    let row = seed::insert_split(&o.primary,&s,"AAPL","2026-09-02 00:00:00",Some("2:1"));
    o.primary.execute("UPDATE account_transactions SET base_amount=10,raw_data=?1 WHERE id=?2",params![json!({"corporate_action":"split","qty":"-10","split_ratio":"2:1","merged_activity_ids":["a","b"]}).to_string(),row]).unwrap();
    o.primary.execute_batch("BEGIN").unwrap();
    for i in 0..5000 { tx.external_id = Some(format!("archived-{i}")); seed::insert_stock_tx(&o.primary,&s,archived,asset,"AAPL",&tx); }
    o.primary.execute_batch("COMMIT").unwrap();
    let now = "2026-09-08T14:00:00.5Z".parse().unwrap();
    seed::fresh_stock_jobs(&o.primary,now);
    let t = ScriptedTransport::from_script(&json!({"GET /v2/clock":[{"status":200,"body":{"timestamp":"2026-09-08T14:00:00Z","is_open":true,"next_open":"2026-09-09T13:30:00Z","next_close":"2026-09-08T20:00:00Z"}}]}));
    let v = AlpacaVenue::new(t.clone(),Urls::for_passphrase(Some("paper")));
    let (out, used) = budget::scope(budget::Limits {steps:20000,held:64000000}, || {
        tick::tick(&o.primary,&v,id,&FixedClock(now),&mut tick::Attempts::default()).now_or_never().expect("scripted replies are ready")
    });
    assert!(matches!(out.unwrap(), tick::TickOutcome::Done {placed:false}));
    let bot = model::load_bot(&o.primary,id).unwrap();
    assert!(bot.transient["rust_split_hold"]["reason"].as_str().unwrap().contains("budget"), "{:?}",bot.transient);
    assert!(used.steps <= 20000);
    assert!(t.posted_orders().is_empty());
}

/// Bot::Restatable#grouped_split_rows on a name a venue lists twice (Alpaca's BTC security beside BTC/USD), as
/// test/models/bot/restatable_test.rb pins it: a report's recorded asset decides; one with none falls back to its name
/// only while the account holds one class under it, and otherwise restates nothing and leaves the split unresolved.
#[test]
fn a_split_of_a_security_never_restates_the_coin_that_shares_its_ticker() {
    use deltabadger::figures::{at::At, budget, db::Order, fill::Raw, splits::{self as rails, Holding}};
    let now = At::from_sql("2026-10-01 00:00:00").unwrap();
    // (holding the coin?, the split row records the security?, the account also bought the security?) → (factors, unresolved)
    for (coin_held, recorded, both_classes, factors, unresolved) in [
        (true, true, false, vec![], false),
        (false, true, false, vec!["2.0"], false),
        (true, false, false, vec!["2.0"], false),
        (true, false, true, vec![], true),
    ] {
        let (_d, o, s) = common::install_alpaca();
        let c = &o.primary;
        c.execute("INSERT INTO assets (external_id, symbol, name, category, created_at, updated_at) VALUES ('BTC.US', 'BTC', 'BTC', 'Stock', ?1, ?1)", [T]).unwrap();
        let stock = c.last_insert_rowid();
        c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
                   minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) \
                   VALUES (?1, 'BTC', 'BTC', 'USD', ?2, ?3, 9, 2, 2, '0.000000001', '1', 1, 1, ?4, ?5)", params![s.exchange_id, stock, s.quote, T, seed::SYNCED]).unwrap();
        if both_classes {
            c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_asset_id, base_amount, quote_currency, quote_amount, \
                       transacted_at, raw_data, created_at, updated_at) VALUES (?1, ?2, 0, 'BTC', ?3, '10', 'USD', '300', '2026-08-01 00:00:00', '{}', ?4, ?4)",
                      params![s.user_id, s.exchange_id, stock, T]).unwrap();
        }
        c.execute("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_asset_id, base_amount, transacted_at, raw_data, created_at, updated_at) \
                   VALUES (?1, ?2, 15, 'BTC', ?3, '0', '2026-09-01 00:00:00', ?4, ?5, ?5)",
                  params![s.user_id, s.exchange_id, recorded.then_some(stock), r#"{"corporate_action":"split","split_ratio":"2:1"}"#, T]).unwrap();
        let held = if coin_held { s.btc } else { stock };
        let orders = [Order { id: 1, at: At::from_sql("2026-08-15 00:00:00").unwrap(), exchange_id: Some(s.exchange_id), raw: Raw::new(None, None, None, None),
                              base: Some("BTC".into()), asset_id: Some(held), sell: false, buy: true, closed: true, kind: "REGULAR".into() }];
        let holdings = [Holding { key: "BTC".into(), asset_id: Some(held), strings: vec!["BTC".into()] }];
        let case = (coin_held, recorded, both_classes);
        let events = budget::within(|| rails::events(c, s.user_id, &orders, &holdings, now)).unwrap();
        assert_eq!(events.iter().map(|e| e.factor.to_s_f()).collect::<Vec<_>>(), factors, "{case:?}");
        assert_eq!(budget::within(|| rails::unresolved(c, s.user_id, &orders, &holdings, now)).unwrap(), unresolved, "{case:?}");
    }
}
