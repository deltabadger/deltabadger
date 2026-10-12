mod common;
use common::seed::{self, BotSpec};
use deltabadger::{engine::{model, staleness}, jobs, sync};
use chrono::{DateTime, Utc, Duration};
use std::rc::Rc;
use serde_json::json;

fn now() -> DateTime<Utc> { "2026-09-01T14:00:00Z".parse().unwrap() }

#[test]
fn ledger_jobs_implement_the_merged_scheduler_contract() {
    let (_d, o, s) = common::install_alpaca();
    let registered: Vec<Box<dyn jobs::Job>> = sync::jobs::register(&o.primary,
        &deltabadger::venue::alpaca::LiveFactory::new(), Rc::new(sync::balances::NoPrices)).unwrap();
    assert_eq!(registered.len(), 3);
    assert_eq!(registered[0].spec().name, "ledger_sync");
    assert_eq!(registered[0].spec().scope.as_deref(), Some(s.api_key_id.to_string().as_str()));
    assert_eq!(registered[1].spec().name, "balance_sync");
    assert_eq!((registered[2].spec().name,registered[2].spec().scope,registered[2].spec().schedule), ("credential_scope_factory",None,None));
}

#[test]
fn stock_freshness_uses_completed_stock_jobs_and_scoped_ledger_success() {
    let (_d, o, s) = common::install_alpaca();
    let (asset, _) = seed::add_alpaca_stock(&o.primary, &s, "AAPL");
    let bot = model::load_bot(&o.primary, seed::insert_bot(&o.primary, &s,
        &BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&[(asset, 1.0)]))).unwrap();
    jobs::state::record_success(&o.primary, jobs::reference::STOCKS, None, now()).unwrap();
    let scope = s.api_key_id.to_string();
    o.primary.execute("DELETE FROM app_configs WHERE key LIKE 'rust_job.ledger_sync:%'", []).unwrap();
    assert_eq!(staleness::stale(&o.primary, &bot, now()).unwrap().unwrap().source, "Alpaca account ledger");
    jobs::state::record_run(&o.primary, "ledger_sync", Some(&scope), now()).unwrap();
    assert!(staleness::stale(&o.primary, &bot, now()).unwrap().is_some(), "NothingNew is not complete");
    jobs::state::record_success(&o.primary, "ledger_sync", Some("999"), now()).unwrap();
    assert!(staleness::stale(&o.primary, &bot, now()).unwrap().is_some(), "another key cannot clear it");
    jobs::state::record_success(&o.primary, "ledger_sync", Some(&scope), now()).unwrap();
    assert!(staleness::stale(&o.primary,&bot,now()).unwrap().is_some(),"a timestamp without the ledger producer is stale");
    let version=model::credential_version_by_id(&o.primary,s.api_key_id).unwrap().unwrap();
    model::credential_write(&o.primary,&Some(version.clone()),|tx|sync::cache::record_ledger(tx,s.api_key_id,&version,now())).unwrap();
    assert!(staleness::stale(&o.primary, &bot, now()).unwrap().is_none());
    assert!(staleness::stale(&o.primary, &bot, now()+Duration::hours(49)).unwrap().is_none());
    assert!(staleness::stale(&o.primary, &bot, now()+Duration::hours(49)+Duration::seconds(1)).unwrap().is_some());
    jobs::state::mark_incomplete(&o.primary, "ledger_sync", Some(&scope), now()).unwrap();
    assert!(staleness::stale(&o.primary, &bot, now()).unwrap().is_some(), "partial ledger stays unsafe even with an older complete run");
    jobs::state::record_success(&o.primary, "ledger_sync", Some(&scope), now()).unwrap();
    jobs::state::record_success(&o.primary, jobs::reference::STOCKS, None, now()-Duration::hours(50)).unwrap();
    o.primary.execute("UPDATE exchange_assets SET updated_at = '2026-09-01 14:00:00'", []).unwrap();
    assert_eq!(staleness::stale(&o.primary, &bot, now()).unwrap().unwrap().source, "Alpaca stock tickers", "partial row stamps do not freshen the tick");
}

/// A reference write is refused only for what it adds: another running bot already outside the slice (a legacy asset with
/// no instrument_type) does not block a harmless write to an eligible bot's asset, and a write that gives a running bot a
/// new reason is still rolled back.
#[test]
fn a_reference_write_is_refused_for_what_it_adds_not_for_what_the_install_already_had() {
    use deltabadger::jobs::import::{in_transaction, Touched};
    let (_d, o, s) = common::install_alpaca();
    let (good, _) = seed::add_alpaca_stock(&o.primary, &s, "AAPL");
    let (legacy, _) = seed::add_alpaca_stock(&o.primary, &s, "GONE");
    o.primary.execute("UPDATE assets SET external_id = 'alpaca_9f', instrument_type = NULL WHERE id = ?1", [legacy]).unwrap();
    seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&[(good, 1.0)]));
    seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&[(legacy, 1.0)]));
    assert!(deltabadger::engine::eligibility::check_install(&o.primary).unwrap().refusal().is_err(), "the install starts outside the slice");
    let write = |sql: &'static str, id: i64| in_transaction(&o.primary, &seed::cipher(), "test", move |tx| {
        tx.execute(sql, [id]).map_err(|e| e.to_string())?;
        Ok(((), Touched::Assets(vec![id])))
    });
    write("UPDATE assets SET name = 'Apple' WHERE id = ?1", good).expect("a harmless write is not blocked by another bot's old problem");
    let refused = write("UPDATE assets SET instrument_type = 'tokenized' WHERE id = ?1", good);
    assert!(refused.is_err_and(|e| e.contains("would stop a running bot")), "a new reason for a running bot is refused");
    let instrument: String = o.primary.query_row("SELECT instrument_type FROM assets WHERE id = ?1", [good], |r| r.get(0)).unwrap();
    assert_eq!(instrument, "stock");
    let refused = write("UPDATE assets SET category = 'Cryptocurrency', instrument_type = 'stock' WHERE id = ?1", legacy);
    assert!(refused.is_err(), "a changed reason on the already-refused bot is new too");
}

fn guarded(o: &deltabadger::store::Opened, sql: &'static str, id: i64) -> Result<(), String> {
    deltabadger::jobs::import::in_transaction(&o.primary, &seed::cipher(), "test", move |tx| {
        tx.execute(sql, [id]).map_err(|e| e.to_string())?;
        Ok(((), deltabadger::jobs::import::Touched::Assets(vec![id])))
    })
}

/// A basket already refused for one member (a legacy asset with no instrument_type) must not mask the same damage dealt to
/// another member: the finding names the member.
#[test]
fn a_refused_member_does_not_mask_the_same_damage_to_another_member() {
    let (_d, o, s) = common::install_alpaca();
    let (a, _) = seed::add_alpaca_stock(&o.primary, &s, "AAA");
    let (b, _) = seed::add_alpaca_stock(&o.primary, &s, "BBB");
    o.primary.execute("UPDATE assets SET external_id = 'alpaca_a1', instrument_type = NULL WHERE id = ?1", [a]).unwrap();
    seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&[(a, 0.5), (b, 0.5)]));
    let out = guarded(&o, "UPDATE assets SET instrument_type = NULL WHERE id = ?1", b);
    assert!(out.is_err_and(|e| e.contains("would stop a running bot")), "B's classification loss is new damage");
    let instrument: String = o.primary.query_row("SELECT instrument_type FROM assets WHERE id = ?1", [b], |r| r.get(0)).unwrap();
    assert_eq!(instrument, "stock");
}

/// A bot that cannot be read (here: an unparseable start_at) must not mask new damage to its own members, nor to another
/// bot's; its finding keeps the read error.
#[test]
fn an_unreadable_bot_does_not_mask_new_damage() {
    let (_d, o, s) = common::install_alpaca();
    let (a, _) = seed::add_alpaca_stock(&o.primary, &s, "AAA");
    let (b, _) = seed::add_alpaca_stock(&o.primary, &s, "BBB");
    let (c, _) = seed::add_alpaca_stock(&o.primary, &s, "CCC");
    seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&[(a, 0.5), (b, 0.5)])
        .with("start_time_enabled", json!(true)).with("start_at", json!("not a time")));
    seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&[(c, 1.0)]));
    let report = deltabadger::engine::eligibility::check_install(&o.primary).unwrap();
    assert_eq!(report.unreadable.len(), 1, "{:?}", report.unreadable);
    guarded(&o, "UPDATE assets SET name = 'Harmless' WHERE id = ?1", a).expect("a harmless write is not blocked");
    assert!(guarded(&o, "UPDATE assets SET instrument_type = NULL WHERE id = ?1", b).is_err(), "the unreadable bot's other member");
    assert!(guarded(&o, "UPDATE assets SET instrument_type = NULL WHERE id = ?1", c).is_err(), "another bot's member");
}
