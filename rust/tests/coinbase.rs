//! Coinbase safety contracts and D-CB-SIZE-IN-QUOTE / D-CB-PAGING.
mod common;
use deltabadger::ruby::BigDec;
use deltabadger::sync::activities::Raw;
use deltabadger::sync::coinbase_ledger::fill;
use deltabadger::venue::coinbase::{CoinbaseVenue, ORDERS, URL};
use deltabadger::venue::http::ScriptedTransport;
use deltabadger::venue::{NewOrder, OrderKind, Side, Venue, VenueError};
use serde_json::{json, Value};
fn bd(s: &str) -> BigDec {
    BigDec::parse(s).unwrap()
}
pub fn raw_fill(product: &str, size: &str, price: &str, fee: &str, quote: bool) -> Value {
    json!({"product_id":product,"size":size,"price":price,"commission":fee,"size_in_quote":quote,"side":"BUY","trade_id":"t1","entry_id":"e1","trade_type":"FILL","trade_time":"2026-09-20T01:00:00Z"})
}
#[test]
fn quote_sized_fills_book_the_quote_spent_and_base_received() {
    for (product, size, price, fee) in [
        ("ETH-EUR", "9.947791211", "2385.55", "0.049738956055"),
        ("BTC-EUR", "14.9262115275", "75480.21", "0.0746310576375"),
    ] {
        let rows = fill(Raw::from_value(raw_fill(product, size, price, fee, true))).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].base_amount, bd(size).div(&bd(price)).unwrap());
        assert_eq!(rows[0].quote_amount, Some(bd(size)));
        assert_eq!(rows[0].fee_amount, Some(bd(fee)));
        let rows = fill(Raw::from_value(raw_fill(product, size, price, fee, false))).unwrap();
        assert_eq!(rows[0].base_amount, bd(size));
        assert_eq!(rows[0].quote_amount, Some(&bd(size) * &bd(price)));
    }
}
fn raw_order(id: &str) -> Value {
    json!({"order_id":id,"product_id":"BTC-EUR","order_type":"MARKET","side":"BUY","status":"FILLED","average_filled_price":"100","filled_size":"0.1","total_value_after_fees":"10.1","outstanding_hold_amount":"0","order_configuration":{"market_market_ioc":{"quote_size":"10"}}})
}
#[tokio::test]
async fn batch_poll_follows_the_second_page_before_reporting_missing() {
    let t = ScriptedTransport::from_script(
        &json!({format!("GET {ORDERS}"):[{"status":200,"body":{"orders":[raw_order("a")],"has_next":true,"cursor":"next"}},{"status":200,"body":{"orders":[raw_order("b")],"has_next":false}}]}),
    );
    let v = CoinbaseVenue::new(t.clone(), URL);
    let out = v.orders(&["a".into(), "b".into()]).await.unwrap();
    assert_eq!(
        out.iter().map(|o| o.txid.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(t.requests().len(), 2);
}
#[tokio::test]
async fn only_proven_rejections_clear_placement() {
    let order = NewOrder {
        pair: "BTC-EUR".into(),
        side: Side::Buy,
        kind: OrderKind::Market,
        volume: "10".into(),
        quote_volume: true,
        cl_ord_id: "5f1c1f3e-2b8f-4c4e-9a59-0d9c3f1b6a01".into(),
        deadline: "2026-09-20T02:00:00Z".parse().unwrap(),
        day: false,
    };
    for (status, body) in [
        (500, json!({"success":false})),
        (200, json!({})),
        (200, json!({"success":true})),
        (200, json!("maintenance")),
        (
            200,
            json!({"success":false,"error_response":{"error":"UNKNOWN_FAILURE_REASON"}}),
        ),
    ] {
        let t = ScriptedTransport::from_script(
            &json!({"POST /api/v3/brokerage/orders":[{"status":status,"body":body}]}),
        );
        assert!(matches!(
            CoinbaseVenue::new(t, URL).add_order(&order).await,
            Err(VenueError::Ambiguous(_))
        ));
    }
    let t = ScriptedTransport::from_script(
        &json!({"POST /api/v3/brokerage/orders":[{"status":200,"body":{"success":false,"error_response":{"error":"INSUFFICIENT_FUND","message":"Insufficient balance in source account"}}}]}),
    );
    assert!(matches!(
        CoinbaseVenue::new(t, URL).add_order(&order).await,
        Err(VenueError::Rejected(_))
    ));
}
#[test]
fn startup_repair_is_scoped_atomic_and_idempotent() {
    let (_dir, o, s) = common::install_alpaca();
    let c = &o.primary;
    c.execute(
        "UPDATE exchanges SET type='Exchanges::Coinbase' WHERE id=?1",
        [s.exchange_id],
    )
    .unwrap();
    let raw = raw_fill("ETH-EUR", "9.947791211", "2385.55", "0.049738956055", true).to_string();
    let base = bd("9.947791211").div(&bd("2385.55")).unwrap().to_s_f();
    for (id, b, q) in [
        (901, "9.947791211", "23731"),
        (902, base.as_str(), "9.947791211"),
    ] {
        c.execute("INSERT INTO account_transactions(id,user_id,exchange_id,entry_type,base_currency,base_amount,quote_currency,quote_amount,fee_currency,fee_amount,raw_data,transacted_at,created_at,updated_at) VALUES(?1,?2,?3,0,'ETH',?4,'EUR',?5,'EUR',0.049738956055,?6,'2026-09-20','2026-09-20','2026-09-20')",rusqlite::params![id,s.user_id,s.exchange_id,b,q,raw]).unwrap();
    }
    c.execute(
        "INSERT INTO exchanges(id,type,name,created_at,updated_at) VALUES(999,'Exchanges::Kraken','Kraken','2026-09-20','2026-09-20')",
        [],
    )
    .unwrap();
    c.execute("INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_amount,raw_data,transacted_at,created_at,updated_at) VALUES(?1,999,0,'ETH',9.947791211,?2,'2026-09-20','2026-09-20','2026-09-20')",rusqlite::params![s.user_id,raw]).unwrap();
    let swap = raw_fill("ETH-BTC", "0.01", "0.03", "0.00005", true).to_string();
    for (id, currency, amount, kind) in [(904, "ETH", "0.01", 2), (905, "BTC", "0.0003", 3)] {
        c.execute("INSERT INTO account_transactions(id,user_id,exchange_id,entry_type,base_currency,base_amount,raw_data,transacted_at,created_at,updated_at) VALUES(?1,?2,?3,?7,?4,?5,?6,'2026-09-20','2026-09-20','2026-09-20')",rusqlite::params![id,s.user_id,s.exchange_id,currency,amount,swap,kind]).unwrap();
    }
    c.execute("INSERT INTO account_transactions(id,user_id,exchange_id,entry_type,base_currency,base_amount,raw_data,transacted_at,created_at,updated_at) VALUES(908,?1,?2,2,'ETH',?3,?4,'2026-09-20','2026-09-20','2026-09-20')", rusqlite::params![s.user_id,s.exchange_id,bd("0.01").div(&bd("0.03")).unwrap().to_s_f(),swap]).unwrap();
    for id in [906, 907] {
        let mut untouched = raw_fill("ETH-EUR", "2", "3000", "1", false);
        if id == 907 {
            untouched.as_object_mut().unwrap().remove("size_in_quote");
        }
        c.execute("INSERT INTO account_transactions(id,user_id,exchange_id,entry_type,base_currency,base_amount,quote_currency,quote_amount,raw_data,transacted_at,created_at,updated_at) VALUES(?1,?2,?3,0,'ETH',2,'EUR',6000,?4,'2026-09-20','2026-09-20','2026-09-20')",rusqlite::params![id,s.user_id,s.exchange_id,untouched.to_string()]).unwrap();
    }
    let now = "2026-10-21T00:00:00Z".parse().unwrap();
    for key in [
        deltabadger::tracker::cache::key(s.user_id),
        deltabadger::tracker::backfill::history_key(s.user_id),
        deltabadger::tracker::backfill::price_key(s.user_id),
    ] {
        deltabadger::app_config::set_plain(c, &key, "old", now).unwrap();
    }
    c.execute("INSERT INTO portfolio_snapshots(user_id,date,created_at,updated_at) VALUES(?1,'2026-09-20','2026-09-20','2026-09-20')",[s.user_id]).unwrap();
    c.execute("INSERT INTO portfolio_venue_snapshots(user_id,exchange_id,date,created_at,updated_at) VALUES(?1,?2,'2026-09-20','2026-09-20','2026-09-20')",rusqlite::params![s.user_id,s.exchange_id]).unwrap();
    // A cancelled startup rolls back the amounts, derived invalidation and marker together.
    {
        let tx = c.unchecked_transaction().unwrap();
        assert_eq!(
            deltabadger::sync::coinbase_repair::run(&tx, now).unwrap(),
            3
        );
        tx.rollback().unwrap();
    }
    assert!(
        deltabadger::app_config::get_plain(c, deltabadger::sync::coinbase_repair::FLAG)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        c.query_row("SELECT count(*) FROM portfolio_snapshots", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );

    let tx = c.unchecked_transaction().unwrap();
    assert_eq!(
        deltabadger::sync::coinbase_repair::run(&tx, now).unwrap(),
        3
    );
    tx.commit().unwrap();
    let read = |id| {
        c.query_row(
            "SELECT base_amount,quote_amount,updated_at FROM account_transactions WHERE id=?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, rusqlite::types::Value>(0)?,
                    r.get::<_, rusqlite::types::Value>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .unwrap()
    };
    let stored = |amount: &str| {
        c.query_row("SELECT CAST(?1 AS NUMERIC)", [amount], |r| {
            r.get::<_, rusqlite::types::Value>(0)
        })
        .unwrap()
    };
    assert_eq!(read(901).0, stored(&base));
    assert_eq!(read(901).1, stored("9.947791211"));
    assert_eq!(read(902).2, "2026-09-20");
    assert_eq!(
        read(908).2,
        "2026-09-20",
        "a correctly stored repeating quotient is untouched"
    );
    assert_eq!(
        read(904).0,
        stored(&bd("0.01").div(&bd("0.03")).unwrap().to_s_f())
    );
    assert_eq!(read(905).0, stored("0.01"));
    assert_eq!(read(904).1, rusqlite::types::Value::Null);
    assert_eq!(read(905).1, rusqlite::types::Value::Null);
    for id in [906, 907] {
        assert_eq!(
            read(id),
            (stored("2"), stored("6000"), "2026-09-20".to_string())
        );
    }
    let (fee, saved_raw): (rusqlite::types::Value, String) = c
        .query_row(
            "SELECT fee_amount,raw_data FROM account_transactions WHERE id=901",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(fee, stored("0.049738956055"));
    assert_eq!(saved_raw, raw);
    for key in [
        deltabadger::tracker::cache::key(s.user_id),
        deltabadger::tracker::backfill::history_key(s.user_id),
        deltabadger::tracker::backfill::price_key(s.user_id),
    ] {
        assert!(deltabadger::app_config::get_plain(c, &key)
            .unwrap()
            .is_none());
    }
    for table in ["portfolio_snapshots", "portfolio_venue_snapshots"] {
        assert_eq!(
            c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    assert_eq!(
        c.query_row(
            "SELECT base_amount FROM account_transactions WHERE exchange_id=999",
            [],
            |r| r.get::<_, rusqlite::types::Value>(0)
        )
        .unwrap(),
        stored("9.947791211")
    );
    let before = c.total_changes();
    let tx = c.unchecked_transaction().unwrap();
    assert_eq!(
        deltabadger::sync::coinbase_repair::run(&tx, now).unwrap(),
        0
    );
    tx.commit().unwrap();
    assert_eq!(c.total_changes(), before);
}

#[tokio::test(flavor = "current_thread")]
async fn repaired_amounts_are_identical_to_fresh_imports() {
    use deltabadger::{crypto::Credentials, engine::FixedClock, jobs::Db, sync::coinbase_ledger};
    for (product, size, price, fee) in [
        ("ETH-EUR", "9.947791211", "2385.55", "0.049738956055"),
        ("BTC-EUR", "14.9262115275", "75480.21", "0.0746310576375"),
        ("ETH-EUR", "18014398509481986", "2", "0"),
        ("ETH-EUR", "1", "3", "0"),
    ] {
        let (_dir, o, s) = common::install_alpaca();
        o.primary
            .execute(
                "UPDATE exchanges SET type='Exchanges::Coinbase' WHERE id=?1",
                [s.exchange_id],
            )
            .unwrap();
        let raw = raw_fill(product, size, price, fee, true);
        let transport = ScriptedTransport::from_script(&json!({
            format!("GET {}", coinbase_ledger::FILLS): [{"status":200,"body":{"fills":[raw]}}],
            format!("GET {}", coinbase_ledger::ACCOUNTS): [{"status":200,"body":{"accounts":[]}}]
        }));
        let db = Db::new(o.primary, common::seed::cipher());
        let credentials = Credentials {
            key: "PKTEST".into(),
            secret: "paper-secret".into(),
            passphrase: Some("paper".into()),
            redaction_values: vec![],
        };
        let clock = FixedClock("2026-09-20T02:00:00Z".parse().unwrap());
        let out = coinbase_ledger::sync(&db, transport, s.api_key_id, &credentials, &clock)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out.imported, 1);
        db.run(move |c, _| {
            // Read SQLite values as well as their byte rendering: text alone can hide distinct REAL values.
            let read = |c: &rusqlite::Connection| c.query_row(
                "SELECT base_amount,quote_amount,CAST(base_amount AS BLOB),CAST(quote_amount AS BLOB) FROM account_transactions WHERE tx_id='t1'",
                [], |r| Ok((r.get::<_, rusqlite::types::Value>(0)?, r.get::<_, rusqlite::types::Value>(1)?, r.get::<_, Vec<u8>>(2)?, r.get::<_, Vec<u8>>(3)?))
            ).unwrap();
            let imported = read(c);
            let now = "2026-10-21T00:00:00Z".parse().unwrap();
            let tx = c.unchecked_transaction().unwrap();
            assert_eq!(deltabadger::sync::coinbase_repair::run(&tx, now).unwrap(), 0, "a fresh {product} import is already correct");
            tx.rollback().unwrap(); // also discard the completion marker
            c.execute("UPDATE account_transactions SET base_amount=?1,quote_amount=?2 WHERE tx_id='t1'", rusqlite::params![size, (&bd(size) * &bd(price)).to_s_f()]).unwrap();
            let tx = c.unchecked_transaction().unwrap();
            assert_eq!(deltabadger::sync::coinbase_repair::run(&tx, now).unwrap(), 1);
            tx.commit().unwrap();
            assert_eq!(read(c), imported, "repaired {product} amounts must match a fresh import exactly");
            Ok(())
        }).await.unwrap();
    }
}

#[test]
fn startup_repair_distinguishes_amounts_hidden_by_float_rounding() {
    for (correct, wrong) in [
        ("9007199254740993", "9007199254740992"),
        ("0.00417002", "0.0041700200000000005"),
    ] {
        for wrong_base in [true, false] {
            let (_dir, o, s) = common::install_alpaca();
            let c = &o.primary;
            c.execute(
                "UPDATE exchanges SET type='Exchanges::Coinbase' WHERE id=?1",
                [s.exchange_id],
            )
            .unwrap();
            let raw = raw_fill("ETH-EUR", correct, "1", "0", true).to_string();
            let (base, quote) = if wrong_base {
                (wrong, correct)
            } else {
                (correct, wrong)
            };
            c.execute("INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_amount,quote_currency,quote_amount,raw_data,transacted_at,created_at,updated_at) VALUES(?1,?2,0,'ETH',?3,'EUR',?4,?5,'2026-09-20','2026-09-20','2026-09-20')", rusqlite::params![s.user_id,s.exchange_id,base,quote,raw]).unwrap();
            let read = |c: &rusqlite::Connection| {
                c.query_row("SELECT base_amount IS CAST(?1 AS NUMERIC) AND quote_amount IS CAST(?1 AS NUMERIC) FROM account_transactions", [correct], |r| r.get::<_, bool>(0)).unwrap()
            };
            assert!(
                !read(c),
                "the fixture must contain a distinct stored amount"
            );
            let tx = c.unchecked_transaction().unwrap();
            assert_eq!(
                deltabadger::sync::coinbase_repair::run(
                    &tx,
                    "2026-10-21T00:00:00Z".parse().unwrap()
                )
                .unwrap(),
                1,
                "correct={correct}, wrong={wrong}, wrong_base={wrong_base}"
            );
            assert!(read(&tx), "both amounts must be stored exactly");
        }
    }
}
#[test]
fn recorded_orders_and_rules_match_rails() {
    use deltabadger::venue::OrderStatus;
    use sha2::Digest;
    let v: Value = serde_json::from_str(include_str!("fixtures/coinbase_vectors.json")).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for (file, pin) in v["sources"].as_object().unwrap() {
        assert_eq!(
            hex::encode(sha2::Sha256::digest(
                std::fs::read(root.join(file)).unwrap()
            )),
            pin.as_str().unwrap(),
            "{file}: run script/rust/repin"
        );
    }
    let orders = v["coinbase_orders"].as_array().unwrap();
    assert_eq!(orders.len(), 2000);
    for c in orders {
        let o = deltabadger::venue::coinbase::parse_order(
            c["raw"]["order_id"].as_str().unwrap(),
            &c["raw"],
        )
        .unwrap();
        let status = match o.status {
            OrderStatus::Unknown => "unknown",
            OrderStatus::Open => "open",
            OrderStatus::Closed => "closed",
            OrderStatus::Cancelled => "cancelled",
            OrderStatus::Failed => "failed",
        };
        assert_eq!(status, c["status"]);
        for (key, got) in [
            ("price", o.price),
            ("amount", o.amount),
            ("quote_amount", o.quote_amount),
            ("amount_exec", Some(o.amount_exec)),
            ("quote_amount_exec", o.quote_amount_exec),
        ] {
            assert_eq!(got, c[key].as_str().map(bd), "{key}: {c}");
        }
        assert_eq!(o.sell, c["side"] == "sell");
        assert_eq!(o.limit, c["order_type"] == "limit_order");
    }
    assert_eq!(
        v["errors"]["insufficient_funds"],
        json!(deltabadger::engine::venue_rules::COINBASE.kinds[0].1)
    );
}
#[test]
fn recorded_sizing_matches_rails() {
    use common::seed::{self, BotSpec};
    use deltabadger::engine::{
        amount::{self, Sizing},
        model,
        venue_rules::COINBASE,
    };
    let (_d, o, s) = common::install();
    let market = model::load_bot(
        &o.primary,
        seed::insert_bot(
            &o.primary,
            &s,
            &BotSpec::weekly(60.0, "2026-09-01 10:00:00"),
        ),
    )
    .unwrap();
    let limit = model::load_bot(
        &o.primary,
        seed::insert_bot(
            &o.primary,
            &s,
            &BotSpec::weekly(60.0, "2026-09-01 10:00:00")
                .with("limit_ordered", json!(true))
                .with("limit_order_pcnt_distance", json!(0.0025)),
        ),
    )
    .unwrap();
    let v: Value = serde_json::from_str(include_str!("fixtures/coinbase_vectors.json")).unwrap();
    let cases = v["coinbase_sizing"].as_array().unwrap();
    assert_eq!(cases.len(), 2728);
    for c in cases {
        let shape = &v["coinbase_tickers"][c["ticker"].as_u64().unwrap() as usize];
        let int = |k: &str| shape[k].as_str().unwrap().parse().unwrap();
        let t = model::Ticker {
            id: 1,
            ticker: "BTC-EUR".into(),
            base_code: "BTC".into(),
            quote_code: "EUR".into(),
            base_symbol: "BTC".into(),
            quote_symbol: "EUR".into(),
            exchange_name: "Coinbase".into(),
            base_asset_id: 1,
            quote_asset_id: 2,
            base_decimals: int("base_decimals"),
            quote_decimals: int("quote_decimals"),
            price_decimals: int("price_decimals"),
            minimum_base_size: bd(shape["minimum_base_size"].as_str().unwrap()),
            minimum_quote_size: bd(shape["minimum_quote_size"].as_str().unwrap()),
            trading_enabled: true,
            available: true,
            crypto: true,
        };
        let bot = if c["order_type"] == "limit_order" {
            &limit
        } else {
            &market
        };
        let (plan, below) = match amount::size(
            bot,
            &t,
            &bd(c["x"].as_str().unwrap()),
            &bd(c["last_or_ask"].as_str().unwrap()),
            &COINBASE,
        )
        .unwrap()
        {
            Sizing::Place(p) => (p, false),
            Sizing::BelowMinimum(p) => (p, true),
            other => panic!("{other:?}"),
        };
        assert_eq!(plan.price, bd(c["price"].as_str().unwrap()));
        assert_eq!(plan.amount, bd(c["amount"].as_str().unwrap()));
        assert_eq!(
            (plan.quote_type, below),
            (
                c["amount_type"] == "quote",
                c["below_minimum"].as_bool().unwrap()
            )
        );
        assert_eq!(
            plan.limit_refusal().unwrap().as_deref(),
            c["refused"].as_str()
        );
    }
}
#[tokio::test]
async fn recovery_finds_only_the_matching_client_and_never_proves_absence() {
    let mut a = raw_order("a");
    a["client_order_id"] = json!("another");
    let mut b = raw_order("b");
    b["client_order_id"] = json!("ours");
    let t = ScriptedTransport::from_script(
        &json!({format!("GET {ORDERS}"):[{"status":200,"body":{"orders":[a],"has_next":true,"cursor":"next"}},{"status":200,"body":{"orders":[b],"has_next":false}}]}),
    );
    let v = CoinbaseVenue::new(t.clone(), URL);
    let since = "2026-09-20T00:00:00Z".parse().unwrap();
    assert_eq!(
        v.order_by_client_id("ours", "BTC-EUR", since)
            .await
            .unwrap()
            .unwrap()
            .txid,
        "b"
    );
    assert_eq!(t.requests().len(), 2);
    assert!(matches!(
        v.order_by_client_id("missing", "BTC-EUR", since).await,
        Err(VenueError::Ambiguous(_))
    ));
}
#[tokio::test]
async fn a_repeated_order_cursor_fails_instead_of_reporting_missing() {
    let t = ScriptedTransport::from_script(
        &json!({format!("GET {ORDERS}"):[{"status":200,"body":{"orders":[],"has_next":true,"cursor":"same"}}]}),
    );
    assert!(CoinbaseVenue::new(t.clone(), URL)
        .orders(&["missing".into()])
        .await
        .is_err());
    assert_eq!(t.requests().len(), 2);
}
