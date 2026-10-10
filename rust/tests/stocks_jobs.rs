mod common;
use common::seed::{self, BotSpec};
use deltabadger::{engine::{model, staleness}, jobs, sync};
use chrono::{DateTime, Utc, Duration};
use std::rc::Rc;

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
