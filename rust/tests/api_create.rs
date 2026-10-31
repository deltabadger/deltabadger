//! 3.0 plan A10 (PLAN-A10-api-create-bots r3): bots created through MCP and REST, beyond what the Rails-recorded grids
//! (pages_api.rb api_create_*, mcp_control.rb create_*) pin: the named divergence with no Rails cell, and two creates
//! racing for one pair.
mod common;
use common::web::{self, TestClock};
use deltabadger::web::bot::create::Kind;
use deltabadger::web::bot_api::{self, Domain, Res};
use deltabadger::web::App;
use serde_json::{json, Map, Value};

const T0: &str = "2026-09-01T12:30:00Z";

fn bots(c: &rusqlite::Connection) -> Vec<(i64, i64)> {
    c.prepare("SELECT id, position FROM bots ORDER BY id").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
}

fn dca(extra: Value) -> Map<String, Value> {
    let mut p = json!({"exchange_name": "Alpaca", "base_asset": "BTC", "quote_asset": "USD", "quote_amount": 25, "interval": "week"}).as_object().unwrap().clone();
    p.extend(extra.as_object().unwrap().clone());
    p
}

async fn create(app: &App, user: i64, p: Map<String, Value>) -> Res {
    let a = app.clone();
    app.db(move |c| bot_api::create(c, &a, user, Kind::Dca, &p)).await.unwrap()
}

/// D-API-CREATE-START-AT-AMBIGUOUS: a wall time the account's zone repeats (Tallinn, 2026-10-25 03:30) is refused as the web's
/// start-time form refuses it, in Rails' own words for a start time it cannot read, and nothing is written. Rails takes the
/// earlier of the two.
#[tokio::test(flavor = "current_thread")]
async fn a_repeated_wall_time_is_refused_and_writes_nothing() {
    let (d, o, s) = common::install_alpaca();
    o.primary.execute("UPDATE users SET time_zone = 'Tallinn'", []).unwrap();
    let before = bots(&o.primary);
    let app = web::app(d.path(), "engine-test-secret", TestClock::at(T0));
    let res = create(&app, s.user_id, dca(json!({"start_at": "2026-10-25T03:30"}))).await;
    assert_eq!((res.status, res.code, res.message.as_str()), (Domain::ValidationFailed, Some("bot_invalid"), "Failed to create bot: Start at must be set"));
    assert_eq!(bots(&o.primary), before, "nothing written");
    // The hour after the change is one wall time: created and scheduled, as Rails does.
    let res = create(&app, s.user_id, dca(json!({"start_at": "2026-10-25T05:30"}))).await;
    assert_eq!(res.status, Domain::Created, "{}", res.message);
    assert_eq!(res.data["started_at"], "2026-10-25T03:30:00Z");
}

/// Two creates for one pair at once: each runs in its own write transaction, so both land, after each other, with their
/// own positions (Rails allows the same pair twice).
#[tokio::test(flavor = "current_thread")]
async fn concurrent_creates_for_one_pair_both_land() {
    let (d, o, s) = common::install_alpaca();
    let before = bots(&o.primary).len();
    let app = web::app(d.path(), "engine-test-secret", TestClock::at(T0));
    let (a, b) = tokio::join!(create(&app, s.user_id, dca(json!({}))), create(&app, s.user_id, dca(json!({}))));
    assert_eq!((a.status, b.status), (Domain::Created, Domain::Created), "{} / {}", a.message, b.message);
    let after = bots(&o.primary);
    assert_eq!(after.len(), before + 2);
    let positions: std::collections::BTreeSet<i64> = after.iter().map(|(_, p)| *p).collect();
    assert_eq!(positions.len(), after.len(), "distinct positions: {after:?}");
}
