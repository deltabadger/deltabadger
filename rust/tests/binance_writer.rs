//! A12 PR 2 writer acceptance: synthetic venue answers and disposable shared-schema databases.
mod common;
use chrono::{DateTime, Utc};
use deltabadger::{app_config, engine::{model, FixedClock, handover}, lease, store, sync::{self, activities::{Entry, Raw}, binance_identity::{Context, Event, Source}, binance_writer as writer}, venue::fake::{FakeFactory, FakeVenue}};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

fn now() -> DateTime<Utc> { "2026-08-18T00:00:00Z".parse().unwrap() }
fn input(source: Source, id: &str, parent: &str, coin: &str) -> writer::Input {
    let raw=json!({"id":id,"txId":"hash","coin":coin,"network":"BTC","address":"addr","addressTag":"","amount":"1",
        "insertTime":1786960800000_i64,"applyTime":"2026-08-17 10:00:00","transactionFee":"0.01","transId":"detail","fromAsset":coin,"transferedAmount":"0.1","serviceChargeAmount":"0.01"});
    let kind=match source {Source::Deposit=>4,Source::Withdrawal=>5,Source::DustOut=>3,Source::DustIn=>2,_=>unreachable!()};
    let dust=matches!(source,Source::DustIn|Source::DustOut);
    writer::Input { event:Event::read(source,&raw,Context{parent:Some(parent),occurred_ms:Some(1786960800000),..Context::default()}).unwrap(), entry:Entry {
        entry_type:kind,base_currency:Some(if kind==2 {"BNB"}else{coin}.into()),base_amount:deltabadger::ruby::BigDec::parse(if kind==2 {"0.1"}else{"1"}).unwrap(),
        quote_currency:None,quote_amount:None,fee_currency:(kind==2).then(||"BNB".into()),fee_amount:(kind==2).then(||deltabadger::ruby::BigDec::parse("0.01").unwrap()),
        tx_id:Some(if dust {format!("dust-detail-{}",if kind==2 {"in"}else{"out"})}else{"hash".into()}),group_id:dust.then(||"dust_detail".into()),description:None,
        transacted_at:Some("2026-08-17T10:00:00Z".parse().unwrap()),raw:Raw::from_value(raw) } }
}
fn pair(parent: &str, coin: &str) -> Vec<writer::Input> { [Source::DustOut,Source::DustIn].into_iter().map(|s|input(s,"unused",parent,coin)).collect() }
fn key(c: &Connection, seed: &common::seed::Seeded) -> sync::Key {
    c.execute("UPDATE exchanges SET type='Exchanges::Binance' WHERE id=?1",[seed.exchange_id]).unwrap();
    sync::load_key_of(c,seed.api_key_id,&["Exchanges::Binance"]).unwrap()
}
fn claim(c: &Connection, path: &std::path::Path) -> lease::EngineLock {
    let lock=lease::lock(&store::Paths::from_env(&|_|None,path),now()).unwrap();
    lease::claim(&lock,c,&common::seed::cipher(),"test",now()).unwrap();lock
}
fn write(c: &Connection, key: &sync::Key, rows: &[writer::Input]) -> Result<sync::ledger::Outcome,sync::SyncError> {
    let tx=model::immediate(c)?;
    let out=writer::store(&model::fence_versions(&tx,&[])?,&common::seed::cipher(),key,rows,now())?;
    tx.commit()?;Ok(out)
}
fn state(c: &Connection) -> Value {
    let tables=["account_transactions","app_configs","portfolio_snapshots","portfolio_venue_snapshots","api_keys","transactions"];
    json!(tables.map(|table| { let mut s=c.prepare(&format!("SELECT * FROM {table} ORDER BY id")).unwrap(); let n=s.column_count();
        s.query_map([],|r|Ok((0..n).map(|i|format!("{:?}",r.get_ref(i).unwrap())).collect::<Vec<_>>())).unwrap().collect::<Result<Vec<_>,_>>().unwrap() }))
}
fn count(c: &Connection) -> i64 { c.query_row("SELECT count(*) FROM account_transactions",[],|r|r.get(0)).unwrap() }
fn csv(rows: &[writer::Input]) -> Vec<Entry> { rows.iter().map(|r| {let mut e=r.entry.clone();e.tx_id=None;e.group_id=Some(format!("dustcsv_{}",e.raw.value["fromAsset"].as_str().unwrap()));
    if e.entry_type==2 { e.base_amount=deltabadger::ruby::BigDec::parse("0.09").unwrap();e.fee_amount=None;e.fee_currency=None; } e.raw=Raw::from_value(json!({"operation":"Small Assets Exchange BNB"}));e }).collect() }
fn file(c: &Connection, key: &sync::Key, rows: &[Entry]) -> Result<sync::ledger::Outcome,sync::SyncError> {
    let tx=model::immediate(c)?;
    let out=writer::store_csv(&model::fence_versions(&tx,&[])?,&common::seed::cipher(),key,rows,now())?;
    tx.commit()?;Ok(out)
}

#[test]
fn csv_fallback_preserves_file_multiplicity_and_repeat_imports() {
    for existing in [0,1] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
        let mut entry=input(Source::Deposit,"d1","","BTC").entry;entry.tx_id=None;
        entry.raw=Raw::from_value(json!({"operation":"Deposit"}));
        if existing==1 {assert_eq!(file(c,&key,std::slice::from_ref(&entry)).unwrap().imported,1);}
        let entries=[entry.clone(),entry];let out=file(c,&key,&entries).unwrap();
        assert_eq!((out.imported,out.duplicates,out.skipped),(2-existing,existing,0));
        assert_eq!(count(c),2,"identical ungrouped rows retain file-level multiplicity");
        let before=state(c);let out=file(c,&key,&entries).unwrap();
        assert_eq!((out.imported,out.duplicates,out.skipped),(0,2,0));assert_eq!(state(c),before);
    }
}

#[test]
fn csv_ambiguous_non_dust_identity_refuses_without_writes() {
    for (grouped,first_matched) in [(false,false),(false,true),(true,false),(true,true)] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
        let rows=[input(Source::Deposit,"d1","","BTC"),input(Source::Deposit,"d2","","BTC"),input(Source::Deposit,"d3","","ETH")];
        assert_eq!(write(c,&key,&rows).unwrap().imported,3);
        let mut entries=vec![rows[2].entry.clone(),rows[0].entry.clone()];
        if !first_matched {entries[0].base_currency=Some("LTC".into());}
        for e in &mut entries {e.tx_id=None;e.group_id=grouped.then(||"csv_group".into());e.raw=Raw::from_value(json!({"operation":"Deposit"}));}
        let before=state(c);let err=file(c,&key,&entries).expect_err("several retained identities must refuse the CSV");
        assert_eq!(err.0,"Binance A12 unresolved: ambiguous CSV identity");assert_eq!(state(c),before);
    }
}

#[test]
fn collision_siblings_reorder_and_later_pages_are_noops() {
    for sources in [[Source::Deposit,Source::Deposit],[Source::Withdrawal,Source::Withdrawal],[Source::Deposit,Source::Withdrawal]] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
        let a=input(sources[0],"d1","","BTC");let b=input(sources[1],"d2","","BTC");
        assert_eq!(write(c,&key,&[a.clone(),b.clone()]).unwrap().imported,2);
        let before=state(c);
        assert_eq!(write(c,&key,&[b.clone(),a.clone()]).unwrap().imported,0);
        for row in [a,b] {assert_eq!(write(c,&key,&[row]).unwrap().imported,0);}
        assert_eq!(state(c),before,"repeat must not touch metadata timestamps either");
    }
}

#[test]
fn dust_groups_and_equal_bnb_csv_are_whole_pair_decisions() {
    for parents in [["p1","p1"],["p1","p2"]] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
        let mut rows=pair(parents[0],"BTC");rows.extend(pair(parents[1],"ETH"));
        assert_eq!(write(c,&key,&rows).unwrap().imported,4);
        assert_eq!(c.query_row("SELECT count(DISTINCT group_id) FROM account_transactions",[],|r|r.get::<_,i64>(0)).unwrap(),2);
        let mut csv=csv(&rows);assert_eq!(file(c,&key,&csv).unwrap().imported,0);let stamped=state(c);
        csv.reverse();assert_eq!(file(c,&key,&csv).unwrap().imported,0);assert_eq!(state(c),stamped);assert_eq!(count(c),4);
        assert!(file(c,&key,&csv[..1]).is_err());assert_eq!(state(c),stamped,"partial pair never stamps or inserts");
    }
}

// PR 3/4 will replace this seeded rollback with the operator command. Metadata is never erased.
fn suppress(c: &Connection, coin: &str) {
    let mut s=c.prepare("SELECT key,value FROM app_configs WHERE key LIKE 'rust_binance_a12_v1:bucket:%'").unwrap();
    let buckets=s.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).unwrap().collect::<Result<Vec<_>,_>>().unwrap();drop(s);
    let tx=c.unchecked_transaction().unwrap();let mut owners=std::collections::HashSet::new();
    for (key,text) in buckets {let mut v:Value=serde_json::from_str(&text).unwrap();for b in v["bindings"].as_array_mut().unwrap() {
        if b["raw"]["fromAsset"]==coin || b["raw"]["coin"]==coin {
            owners.insert(b["after"]["user_id"].as_i64().unwrap());
            tx.execute("DELETE FROM account_transactions WHERE id=?1",[b["row_id"].as_i64().unwrap()]).unwrap();b["suppressed"]=json!(true);b["suppression_reason"]=json!("seeded operator rollback");
        }
    } app_config::set_plain(&tx,&key,&v.to_string(),now()).unwrap();}
    for owner in owners {
        for key in [deltabadger::tracker::cache::key(owner),deltabadger::tracker::backfill::history_key(owner),deltabadger::tracker::backfill::price_key(owner)] {tx.execute("DELETE FROM app_configs WHERE key=?1",[key]).unwrap();}
        for table in ["portfolio_snapshots","portfolio_venue_snapshots"] {tx.execute(&format!("DELETE FROM {table} WHERE user_id=?1"),[owner]).unwrap();}
    }tx.commit().unwrap();
}

#[test]
fn suppression_precedes_sync_csv_and_equal_bnb_stamped_anchor_after_restart() {
    let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let lock=claim(c,dir.path());
    let anchor=pair("p1","BTC");let sibling=pair("p2","ETH");let mut all=anchor.clone();all.extend(sibling.clone());
    write(c,&key,&all).unwrap();file(c,&key,&csv(&anchor)).unwrap();
    c.execute("UPDATE account_transactions SET raw_data=json_set(raw_data,'$.file_twin_at','2026-08-17T10:00:00.000000Z') WHERE json_extract(raw_data,'$.fromAsset')='BTC'",[]).unwrap();suppress(c,"ETH");
    let before=state(c);drop(o);let o=store::open(&store::Paths::from_env(&|_|None,dir.path())).unwrap();let c=&o.primary;
    for rows in [sibling,all] {assert_eq!(write(c,&key,&rows).unwrap().imported,0);let mut csv=csv(&rows);
        for _ in 0..2 {csv.reverse();assert_eq!(file(c,&key,&csv).unwrap().imported,0);assert_eq!(state(c),before);}}
    assert!(lease::hand_back(&lock,c,&common::seed::cipher(),now()).is_err());
}

#[test]
fn invalidation_and_marker_roll_back_with_failed_insert_and_zero_insert_retry_is_fresh() {
    let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());let rows=pair("p1","BTC");
    for k in [deltabadger::tracker::cache::key(key.user_id),deltabadger::tracker::backfill::history_key(key.user_id),deltabadger::tracker::backfill::price_key(key.user_id)] {app_config::set_plain(c,&k,"stale",now()).unwrap();}
    c.execute("INSERT INTO portfolio_snapshots(user_id,date,created_at,updated_at) VALUES (?1,'2099-01-01','2099-01-01','2099-01-01')",[key.user_id]).unwrap();
    c.execute("INSERT INTO portfolio_venue_snapshots(user_id,exchange_id,date,created_at,updated_at) VALUES (?1,?2,'2099-01-01','2099-01-01','2099-01-01')",params![key.user_id,key.exchange_id]).unwrap();
    let before=state(c);
    c.execute_batch("CREATE TEMP TRIGGER fail_invalidation BEFORE DELETE ON app_configs BEGIN SELECT RAISE(ABORT,'simulated crash'); END").unwrap();
    assert!(write(c,&key,&rows).is_err());assert_eq!(state(c),before);c.execute_batch("DROP TRIGGER fail_invalidation").unwrap();
    assert_eq!(write(c,&key,&rows).unwrap().imported,2);assert!(app_config::get_plain(c,&deltabadger::tracker::cache::key(key.user_id)).unwrap().is_none());
    for table in ["portfolio_snapshots","portfolio_venue_snapshots"] {assert_eq!(c.query_row(&format!("SELECT count(*) FROM {table}"),[],|r|r.get::<_,i64>(0)).unwrap(),0);}
    let after=state(c);assert_eq!(write(c,&key,&rows).unwrap().imported,0);assert_eq!(state(c),after);
}

#[tokio::test]
async fn every_handback_path_refuses_marker_even_after_full_rollback_and_recovery_cleanup() {
    for value in [Some("1"),Some("unsupported"),Some(""),None] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let lock=claim(c,dir.path());
        write(c,&key,&[input(Source::Deposit,"d1","","BTC")]).unwrap();
        c.execute_batch("CREATE TEMP TABLE recovery_rows AS SELECT * FROM account_transactions").unwrap();suppress(c,"BTC");assert_eq!(count(c),0);
        assert!(lease::hand_back(&lock,c,&common::seed::cipher(),now()).is_err(),"all inserted rows rolled back");
        c.execute_batch("INSERT INTO account_transactions SELECT * FROM recovery_rows; DROP TABLE recovery_rows").unwrap();
        c.execute("UPDATE app_configs SET value=json_set(value,'$.bindings[0].suppressed',json('false'),'$.bindings[0].suppression_reason',NULL) WHERE key LIKE 'rust_binance_a12_v1:bucket:%'",[]).unwrap();
        assert!(lease::hand_back(&lock,c,&common::seed::cipher(),now()).is_err(),"recovery cleared the last suppression");
        c.execute("DELETE FROM account_transactions",[]).unwrap();
        c.execute("DELETE FROM app_configs WHERE key LIKE 'rust_binance_a12_v1:bucket:%'",[]).unwrap();
        c.execute("UPDATE app_configs SET value=?1 WHERE key=?2",params![value,writer::MARKER]).unwrap();
        drop(o);let o=store::open(&store::Paths::from_env(&|_|None,dir.path())).unwrap();let before=state(&o.primary);
        let factory=FakeFactory(FakeVenue::new());let cipher=common::seed::cipher();let clock=FixedClock(now());
        assert!(handover::hand_back(&lock,&o,&factory,&cipher,&clock).await.is_err());
        assert!(handover::hand_back_since(&lock,&o,&factory,&cipher,&clock,now()).await.is_err());
        assert!(handover::hand_back_cli(&lock,&o,&factory,&cipher,&clock).await.is_err());
        assert!(lease::hand_back(&lock,&o.primary,&cipher,now()).is_err());assert_eq!(state(&o.primary),before);
        assert_eq!(lease::read(&o.primary,&cipher).unwrap().unwrap()["engine"],"rust");
    }
}

#[test]
fn ownership_loss_corrections_deleted_aliases_and_partial_pairs_refuse_without_writes() {
    let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let lock=claim(c,dir.path());let row=input(Source::Deposit,"d1","","BTC");
    lease::hand_back(&lock,c,&common::seed::cipher(),now()).unwrap();let before=state(c);assert!(write(c,&key,std::slice::from_ref(&row)).is_err());assert_eq!(state(c),before);
    lease::claim(&lock,c,&common::seed::cipher(),"test",now()).unwrap();write(c,&key,std::slice::from_ref(&row)).unwrap();
    c.execute("DELETE FROM account_transactions",[]).unwrap();let before=state(c);assert!(write(c,&key,&[row]).is_err());assert_eq!(state(c),before);
    assert!(write(c,&key,&pair("p1","BTC")[..1]).is_err());assert_eq!(state(c),before);
}

#[test]
fn metadata_only_adoption_preserves_every_existing_column_and_marks_atomically() {
    let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());let rows=pair("p1","BTC");
    write(c,&key,&rows).unwrap();
    // Seed the recorded legacy shape, including a manual value and a retained group.
    c.execute("DELETE FROM app_configs WHERE key LIKE 'rust_binance_a12_v1:%'",[]).unwrap();
    c.execute("UPDATE account_transactions SET group_id='dust_detail',manual_values='{\"price\":\"123.45\"}',transfer_link_rejected=1",[]).unwrap();
    let before=state(c);let ledger=before[0].clone();
    c.execute_batch("CREATE TEMP TRIGGER fail_marker BEFORE INSERT ON app_configs WHEN NEW.key='rust_binance_a12_v1:install_written' BEGIN SELECT RAISE(ABORT,'simulated crash'); END").unwrap();
    assert!(write(c,&key,&rows).is_err());assert_eq!(state(c),before);c.execute_batch("DROP TRIGGER fail_marker").unwrap();
    assert_eq!(write(c,&key,&rows).unwrap().imported,0);assert_eq!(state(c)[0],ledger);assert!(writer::written(c).unwrap());
    let before=state(c);write(c,&key,&rows).unwrap();assert_eq!(state(c),before);
    assert_eq!(write(c,&key,&pair("p2","ETH")).unwrap().imported,2);
    assert_eq!(c.query_row("SELECT count(*) FROM account_transactions WHERE group_id='dust_detail'",[],|r|r.get::<_,i64>(0)).unwrap(),2);
}

#[test]
fn suppressed_non_dust_csv_and_correction_conflicts_never_fall_through() {
    let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());let row=input(Source::Deposit,"d1","","BTC");
    write(c,&key,std::slice::from_ref(&row)).unwrap();
    let mut changed=row.clone();changed.entry.raw=Raw::from_value({let mut raw=row.entry.raw.value.clone();raw["amount"]=json!("2");raw});
    changed.entry.base_amount=deltabadger::ruby::BigDec::from_i64(2);changed.event=Event::read(Source::Deposit,&changed.entry.raw.value,Context::default()).unwrap();
    let before=state(c);assert!(write(c,&key,&[changed.clone()]).is_err());assert_eq!(state(c),before);
    suppress(c,"BTC");let before=state(c);let mut entry=row.entry;entry.tx_id=None;
    assert_eq!(file(c,&key,&[entry]).unwrap().skipped,1);assert_eq!(state(c),before);
    let mut entry=changed.entry;entry.tx_id=None;assert!(file(c,&key,&[entry]).is_err());assert_eq!(state(c),before);
}

#[test]
fn concurrent_first_write_and_handback_serialize_in_both_orders() {
    use std::{sync::mpsc,thread,time::Duration};
    for writer_first in [true,false] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let lock=claim(c,dir.path());let row=input(Source::Deposit,"d1","","BTC");
        let tx=model::immediate(c).unwrap();
        if writer_first {writer::store(&model::fence_versions(&tx,&[]).unwrap(),&common::seed::cipher(),&key,std::slice::from_ref(&row),now()).unwrap();}
        else {lease::hand_back(&lock,&tx,&common::seed::cipher(),now()).unwrap();}
        let path=dir.path().join("production.sqlite3");let (started_tx,started_rx)=mpsc::channel();let (done_tx,done_rx)=mpsc::channel();
        let other=thread::spawn(move || {let c=Connection::open(path).unwrap();c.busy_timeout(Duration::from_secs(5)).unwrap();started_tx.send(()).unwrap();
            let refused=if writer_first {lease::hand_back(&lock,&c,&common::seed::cipher(),now()).is_err()}else{write(&c,&key,&[row]).is_err()};done_tx.send(refused).unwrap();});
        started_rx.recv().unwrap();assert!(done_rx.recv_timeout(Duration::from_millis(30)).is_err(),"the competing transition holds until commit");
        tx.commit().unwrap();assert!(done_rx.recv_timeout(Duration::from_secs(5)).unwrap());other.join().unwrap();
        assert_eq!(writer::written(c).unwrap(),writer_first);assert_eq!(count(c),i64::from(writer_first));
        assert_eq!(lease::read(c,&common::seed::cipher()).unwrap().unwrap()["engine"],if writer_first {"rust"}else{"none"});
    }
}

#[test]
fn unreadable_retained_dust_context_refuses_both_writers_without_panicking() {
    let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());let rows=pair("p1","BTC");write(c,&key,&rows).unwrap();
    c.execute("UPDATE app_configs SET value=json_set(value,'$.bindings[0].event.pair','broken tuple') WHERE key LIKE 'rust_binance_a12_v1:bucket:%'",[]).unwrap();
    let before=state(c);assert!(write(c,&key,&rows).is_err());assert!(file(c,&key,&csv(&rows)).is_err());assert_eq!(state(c),before);
}

#[test]
fn empty_writer_does_not_mark_the_install_or_prevent_handback() {
    let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let lock=claim(c,dir.path());let before=state(c);
    assert_eq!(write(c,&key,&[]).unwrap().imported,0);assert_eq!(state(c),before);assert!(!writer::written(c).unwrap());
    lease::hand_back(&lock,c,&common::seed::cipher(),now()).unwrap();
}

#[test]
fn a_changed_legacy_hash_cannot_reinsert_a_known_or_suppressed_capital_identity() {
    for suppressed in [None,Some(false),Some(true)] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());let row=input(Source::Deposit,"d1","","BTC");
        write(c,&key,std::slice::from_ref(&row)).unwrap();if suppressed==Some(true) {suppress(c,"BTC");}
        if suppressed.is_none() {c.execute("DELETE FROM app_configs WHERE key LIKE 'rust_binance_a12_v1:%'",[]).unwrap();}
        let before=state(c);let mut changed=row;changed.entry.tx_id=Some("changed-hash".into());let mut raw=changed.entry.raw.value.clone();raw["txId"]=json!("changed-hash");
        changed.entry.raw=Raw::from_value(raw);changed.event=Event::read(Source::Deposit,&changed.entry.raw.value,Context::default()).unwrap();
        assert!(write(c,&key,&[changed]).is_err());assert_eq!(state(c),before);
    }
}

#[test]
fn a_nil_id_row_with_the_same_capital_record_id_cannot_be_recreated_at_another_time() {
    let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());let row=input(Source::Deposit,"d1","","BTC");
    write(c,&key,std::slice::from_ref(&row)).unwrap();c.execute("DELETE FROM app_configs WHERE key LIKE 'rust_binance_a12_v1:%'",[]).unwrap();
    c.execute("UPDATE account_transactions SET tx_id=NULL,transacted_at='2020-01-01 00:00:00'",[]).unwrap();
    let before=state(c);assert!(write(c,&key,&[row]).is_err());assert_eq!(state(c),before);
}

#[test]
fn csv_requires_current_raw_identity_and_exactly_two_rows_in_a_retained_dust_group() {
    for mutation in ["UPDATE account_transactions SET raw_data=json_set(raw_data,'$.amount','2')", "UPDATE account_transactions SET group_id=(SELECT group_id FROM account_transactions ORDER BY id LIMIT 1)"] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());let mut rows=pair("p1","BTC");rows.extend(pair("p2","ETH"));
        write(c,&key,&rows).unwrap();c.execute(mutation,[]).unwrap();let before=state(c);assert!(file(c,&key,&csv(&rows)).is_err());assert_eq!(state(c),before);
    }
}


fn import(c:&Connection,key:&sync::Key,format:&str,text:&str)->Result<deltabadger::tracker::import::run::Answer,sync::SyncError> {
    deltabadger::tracker::import::run::import(c,&common::seed::cipher(),key.user_id,format,text.as_bytes(),Some("+00:00"),Some(&key.id.to_string()),now())
}
fn dust_file(coins:&[&str])->String {
    let mut text="User ID,UTC_Time,Account,Operation,Coin,Change,Remark\n".to_string();
    for coin in coins {text+=&format!("1,2026-08-17 10:00:00,Spot,Small Assets Exchange BNB,{coin},-1,{coin}\n1,2026-08-17 10:00:00,Spot,Small Assets Exchange BNB,BNB,0.09,{coin}\n");}text
}
#[test]
fn production_csv_keeps_whole_equal_bnb_pairs_and_suppressions_after_restart() {
    use deltabadger::tracker::import::run::Answer;
    for (existing,net) in [(false,"0.09"),(true,"0.09"),(false,"0.19"),(true,"0.19")] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
        let mut sibling=pair("p2","ETH");
        if net=="0.19" {for i in &mut sibling {let mut raw=i.entry.raw.value.clone();raw["transferedAmount"]=json!("0.2");i.entry.raw=Raw::from_value(raw);
            if i.entry.entry_type==2 {i.entry.base_amount=deltabadger::ruby::BigDec::parse("0.2").unwrap();}
            i.event=Event::read(if i.entry.entry_type==2 {Source::DustIn}else{Source::DustOut},&i.entry.raw.value,Context{parent:Some("p2"),occurred_ms:Some(1786960800000),..Context::default()}).unwrap();}}
        if existing {write(c,&key,&pair("p1","BTC")).unwrap();write(c,&key,&sibling).unwrap();
            c.execute("UPDATE account_transactions SET raw_data=json_set(raw_data,'$.file_twin_at','2026-08-17T10:00:00Z') WHERE json_extract(raw_data,'$.fromAsset')='BTC'",[]).unwrap();}
        let csv=|coins:&[&str]|dust_file(coins).replace("BNB,0.09,ETH",&format!("BNB,{net},ETH"));
        let file=csv(&["BTC","ETH"]);
        let result=import(c,&key,"binance",&file).unwrap();
        assert!(matches!(result,Answer::Written{imported,duplicates,skipped:0,..} if imported==if existing {0}else{4} && duplicates==if existing {4}else{0}),"{result:?}");
        assert!(writer::written(c).unwrap());assert_eq!(count(c),4);
        let before=state(c);let result=import(c,&key,"binance",&csv(&["ETH","BTC"])).unwrap();
        assert!(matches!(result,Answer::Written{imported:0,duplicates:4,..}),"{result:?}");assert_eq!(state(c),before);
        if existing {
            suppress(c,"ETH");let before=state(c);let restarted=Connection::open(dir.path().join("production.sqlite3")).unwrap();
            for coins in [vec!["ETH"],vec!["ETH","BTC"],vec!["BTC","ETH"]] {
                let result=import(&restarted,&key,"binance",&csv(&coins)).unwrap();
                assert!(matches!(result,Answer::Written{imported:0,skipped:2,..}),"{result:?}");assert_eq!(state(c),before);
            }
        }
    }
}

#[tokio::test(flavor="current_thread")]
async fn production_export_import_matches_gross_dust_and_rejects_suppressed_ids() {
    use common::web::{self,Browser,TestClock};
    use deltabadger::{web::{csrf,session},tracker::import::run::Answer};
    let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
    write(c,&key,&pair("p1","BTC")).unwrap();write(c,&key,&pair("p2","ETH")).unwrap();
    c.execute("UPDATE users SET confirmed_at=created_at,setup_completed=1",[]).unwrap();
    let hash:String=c.query_row("SELECT encrypted_password FROM users WHERE id=?1",[s.user_id],|r|r.get(0)).unwrap();
    let app=web::app(dir.path(),web::SECRET,TestClock::at("2026-08-18T00:00:00Z"));
    let data=session::SessionData{user:Some((s.user_id,hash.chars().take(29).collect())),csrf:Some(csrf::new_token()),..Default::default()};
    let mut browser=Browser{cookie:Some(session::seal(&app.keys.session,&data,app.now())),page:None};
    let exported=browser.get(&app,"/tracker/export").await;assert_eq!(exported.status,200,"{}",exported.body);
    let before=state(c);let answer=import(c,&key,"deltabadger",&exported.body).unwrap();
    assert!(matches!(answer,Answer::Written{imported:0,duplicates:4,..}),"{answer:?}");assert_eq!(state(c),before);
    suppress(c,"ETH");let before=state(c);let answer=import(c,&key,"deltabadger",&exported.body).unwrap();
    assert!(matches!(answer,Answer::Written{imported:0,duplicates:2,skipped:2,..}),"{answer:?}");assert_eq!(state(c),before);
    let unbound=deltabadger::web::tracker::CSV_HEADERS.join(",")+"\n2026-08-17T10:00:00Z,deposit,BTC,1,,,,,binance,binance-a12-v1:deposit:unknown,,\n";
    assert!(matches!(import(c,&key,"deltabadger",&unbound).unwrap(),Answer::Refused(_)));assert_eq!(state(c),before);
    let unknown=exported.body.replace("dust-a12-v1-","dust-a12-v1-unknown-");assert_ne!(unknown,exported.body);
    assert!(matches!(import(c,&key,"deltabadger",&unknown).unwrap(),Answer::Refused(_)));assert_eq!(state(c),before);
    let changed=exported.body.replace(",0.1,",",0.2,");assert_ne!(changed,exported.body);
    assert!(matches!(import(c,&key,"deltabadger",&changed).unwrap(),Answer::Refused(_)));assert_eq!(state(c),before);
}

// Prices and tax settings are fixed by the unchanged Rails accounting recorder. The
// quantities, fees, ordering and groups below are read back from the actual database.
fn accounting(c:&Connection,owner:i64,expected:&Value) {
    use deltabadger::{figures::dec::Dec,tracker::{rows,walk::{self,Engine,Row}}};
    let d=|s:&str|Dec::parse(s).unwrap();
    let rows:Vec<_>=rows::load_all(c,owner).unwrap().into_iter().enumerate().map(|(uid,r)| {
        let price=d(match r.base.as_str(){"BTC"=>"10","ETH"=>"20","BNB"=>"100",_=>panic!("unexpected asset")});
        let value=r.quote_amount.clone().unwrap_or_else(||(&r.amount*&price).unwrap());
        let fee=r.fee_amount.as_ref().map(|f|(f*&price).unwrap()).unwrap_or_else(Dec::zero);
        Row{uid,kind:r.kind,base:r.base,amount:r.amount,quote:None,quote_amount:None,fiat_value:value,fee_fiat_value:fee,
            fee_currency:r.fee_currency,fee_amount:r.fee_amount,at:r.at,group:r.group,price_missing:false,value_missing:None,
            exchange:r.venue,linked:false,transfer_fee:None,swap_fiat_cost:None,swap_stable_cost:None,per_share:None,to_exchange:None,orphan:false,opening:false}
    }).collect();
    let mut engine=Engine::new(false);engine.calculate(&rows).unwrap();
    let lots:std::collections::BTreeMap<_,_>=engine.remaining_lots().map(|(asset,venue,lots)| {
        assert!(venue.is_none());(asset,json!(lots.into_iter().map(|(n,c)|[n.to_s_f(),c.to_s_f()]).collect::<Vec<_>>()))
    }).collect();
    assert_eq!(json!(lots),expected["lots"],"exact remaining FIFO lots from stored history");
    let mut held=std::collections::BTreeMap::<String,Dec>::new();
    for r in &rows {for (coin,n) in walk::quantity_moves(r.kind,&r.base,&r.amount,r.fee_currency.as_deref(),r.fee_amount.as_ref(),false,None).unwrap(){let old=held.entry(coin).or_insert_with(Dec::zero);*old=(&*old+&n).unwrap();}}
    assert_eq!(json!(held.into_iter().map(|(k,v)|(k,v.to_s_f())).collect::<std::collections::BTreeMap<_,_>>()),expected["holdings"]);
    assert_eq!(engine.disposals.len(),expected["disposals"].as_array().unwrap().len());
    let mut realised=Dec::zero();
    for (a,e) in engine.disposals.iter().zip(expected["disposals"].as_array().unwrap()) {
        for (field,value) in [("amount",&a.amount),("proceeds",&a.proceeds),("cost_basis",&a.cost_basis),("gain_loss",&a.gain_loss)]{assert_eq!(value.to_s_f(),e[field],"{field}");}
        realised=(&realised+&a.gain_loss).unwrap();
    }
    assert_eq!(realised.to_s_f(),expected["realised"]);
}

fn accounting_history(fixture:&Value,colliding:bool,earn:bool)->(Value,String) {
    let mut state=json!({"deposits":[],"withdrawals":[],"earn":{"locked_rewardsRecord":[],"flexible_subscriptionRecord":[],"flexible_redemptionRecord":[]},"dust":[]});
    let mut file=deltabadger::web::tracker::CSV_HEADERS.join(",")+"\n";
    for (n,r) in fixture["accounting"][0]["rows"].as_array().unwrap().iter().enumerate() {
        let kind=r["entry_type"].as_str().unwrap();let coin=r["base_currency"].as_str().unwrap();let amount=r["base_amount"].as_str().unwrap();
        let at:DateTime<Utc>=r["transacted_at"].as_str().unwrap().parse().unwrap();let ms=at.timestamp_millis();
        match kind {
            "deposit"|"withdrawal" if earn && [3,4].contains(&n)=> {
                let source=if kind=="deposit" {"flexible_redemptionRecord"}else{"flexible_subscriptionRecord"};
                state["earn"][source].as_array_mut().unwrap().push(json!({"asset":coin,"time":ms,"amount":amount,"purchaseAmount":amount,"purchaseId":"purchase","redeemId":"redeem","projectId":"project","productId":"product"}));
            },
            "deposit"|"withdrawal"=> {
                let raw=json!({"id":format!("{n}"),"txId":format!("chain-{n}"),"coin":coin,"network":coin,"address":"addr","addressTag":"","amount":amount,
                    "insertTime":ms,"applyTime":at.format("%Y-%m-%d %H:%M:%S").to_string(),"transactionFee":"0","status":if kind=="deposit" {1}else{6}});
                state[if kind=="deposit" {"deposits"}else{"withdrawals"}].as_array_mut().unwrap().push(raw);
            },
            "staking_reward"=>state["earn"]["locked_rewardsRecord"].as_array_mut().unwrap().push(json!({"positionId":"reward","asset":coin,"time":ms,"rewards":amount})),
            "swap_out" if r["group_id"].as_str().unwrap().starts_with("dust")=>state["dust"].as_array_mut().unwrap().push(json!({"transId":format!("parent-{coin}"),"operateTime":ms,"userAssetDribbletDetails":[{"transId":if colliding {"detail"}else{coin},"fromAsset":coin,"amount":amount,"transferedAmount":"0.1","serviceChargeAmount":"0.01"}]})),
            "swap_in" if coin=="BNB"=>{},
            _=>{let quoted=["buy","sell"].contains(&kind);file+=&format!("{},{kind},{coin},{amount},{},{},,,binance,seed-{n},{},\n",at.to_rfc3339(),if quoted {"USD"}else{""},if quoted {r["fiat_value"].as_str().unwrap()}else{""},r["group_id"].as_str().unwrap_or(""));}
        }
    }
    (state,file)
}

#[tokio::test(flavor="current_thread")]
async fn live_history_csv_and_seeded_rollback_match_exact_recorded_accounting() {
    let fixture:Value=serde_json::from_str(include_str!("fixtures/binance_identity.json")).unwrap();
    for (colliding,earn,file_only) in [(false,false,false),(false,true,false),(true,false,false),(true,true,false),(false,false,true)] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let lock=claim(c,dir.path());
        let (mut state,file)=accounting_history(&fixture,colliding,earn);
        import(c,&key,"deltabadger",&file).unwrap();
        for k in [deltabadger::tracker::cache::key(key.user_id),deltabadger::tracker::backfill::history_key(key.user_id),deltabadger::tracker::backfill::price_key(key.user_id)] {app_config::set_plain(c,&k,"stale",now()).unwrap();}
        c.execute("INSERT INTO portfolio_snapshots(user_id,date,created_at,updated_at) VALUES (?1,'2099-01-01','2099-01-01','2099-01-01')",[key.user_id]).unwrap();
        c.execute("UPDATE api_keys SET last_synced_at='2026-07-31 00:00:00'",[]).unwrap();drop(lock);
        if file_only {state["dust"]=json!([]);}
        let scenario=json!({"parity_scratch":true,"api_key_id":key.id,"credentials":{"key":"test-key","secret":"dGVzdC1zZWNyZXQ="},"steps":[{"kind":"ledger","at":"2026-08-18T00:00:00Z","binance_state":state}]});
        std::fs::write(dir.path().join("scenario.json"),scenario.to_string()).unwrap();
        let out=sync::parity::run(dir.path(),std::sync::Arc::new(common::seed::cipher())).await.unwrap();assert_eq!(out["steps"][0]["raised"],false,"{out}");
        let csv=dust_file(&["BTC","ETH"]).replace("2026-08-17 10:00:00","2026-08-08 00:00:00");
        if file_only {import(c,&key,"binance",&csv).unwrap();}
        accounting(c,key.user_id,&fixture["accounting"][0]);
        sync::parity::run(dir.path(),std::sync::Arc::new(common::seed::cipher())).await.unwrap();
        assert_eq!(c.query_row("SELECT count(*) FROM portfolio_snapshots",[],|r|r.get::<_,i64>(0)).unwrap(),0,"future-dated stale snapshot was deleted atomically");
        rebuild_from_cold(c,dir.path(),key.user_id).await;
        // Venue CSV records net BNB, while the API retained the gross and fee.
        import(c,&key,"binance",&csv).unwrap();accounting(c,key.user_id,&fixture["accounting"][0]);
        if file_only {continue;}
        suppress(c,"ETH");accounting(c,key.user_id,&fixture["accounting"][1]);rebuild_from_cold(c,dir.path(),key.user_id).await;
        for _ in 0..2 {import(c,&key,"binance",&csv).unwrap();accounting(c,key.user_id,&fixture["accounting"][1]);}
        sync::parity::run(dir.path(),std::sync::Arc::new(common::seed::cipher())).await.unwrap();accounting(c,key.user_id,&fixture["accounting"][1]);
    }
}


async fn rebuild_from_cold(c:&Connection,path:&std::path::Path,owner:i64) {
    use deltabadger::tracker::{cache,backfill,jobs,prices::{PriceService,Unfetched},walk};
    assert!(matches!(cache::read(c,owner,None,now()).unwrap(),cache::State::Cold));
    assert!(backfill::wanted(c,&common::seed::cipher(),owner,now().date_naive()).unwrap());
    use common::web::{self,Browser,TestClock};use deltabadger::web::session;
    c.execute("UPDATE users SET confirmed_at=created_at,setup_completed=1",[]).unwrap();
    let hash:String=c.query_row("SELECT encrypted_password FROM users WHERE id=?1",[owner],|r|r.get(0)).unwrap();
    let env=json!({"MARKET_DATA_URL":"https://market-data.example"});
    let app=web::app_in(path,"engine-test-secret",None,env.as_object().unwrap(),TestClock::at("2026-08-18T00:00:00Z"));app.attach_jobs(deltabadger::jobs::Wakers::default()).unwrap();
    let data=session::SessionData{user:Some((owner,hash.chars().take(29).collect())),..Default::default()};
    let mut browser=Browser{cookie:Some(session::seal(&app.keys.session,&data,app.now())),page:None};
    assert_eq!(browser.get(&app,"/tracker").await.status,200);
    let wakes=app.job_wakers().unwrap().pending();
    for job in [jobs::TRACKER_LEDGER,jobs::PORTFOLIO_BACKFILL] {assert!(wakes.contains(&(job.into(),Some(owner.to_string()))),"{job}: {wakes:?}");}
    let inputs=jobs::load_inputs(c,owner).unwrap();let mut no_prices=Unfetched;
    let mut prices=PriceService::new(&inputs.reference,&inputs.fx,now().date_naive(),&mut no_prices);
    let rebuilt=walk::scopes(&inputs.rows,&inputs.deposits,&mut prices,&inputs.ids,deltabadger::figures::at::At::from_utc(now()).unwrap()).unwrap();
    let tx=model::immediate(c).unwrap();assert!(cache::publish(&model::fence_versions(&tx,&[]).unwrap(),owner,&inputs.cache,Some(&rebuilt),now()).unwrap());tx.commit().unwrap();
    let cache::State::Warm(read)=cache::read(c,owner,None,now()).unwrap() else {panic!("the read must use the rebuilt cache")};
    assert_eq!(read.realised,rebuilt.whole.realised);assert_eq!(read.positions.len(),rebuilt.whole.positions.len());
    assert_eq!(browser.get(&app,"/tracker").await.status,200);
    assert!(!app.job_wakers().unwrap().pending()[wakes.len()..].iter().any(|(job,_)|job==jobs::TRACKER_LEDGER));
}

#[test]
fn production_csv_distinguishes_new_same_asset_pairs_by_the_complete_source_leg() {
    use deltabadger::tracker::import::run::Answer;
    let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
    let first=dust_file(&["BTC"]);let second=first.replace("BTC,-1,BTC","BTC,-2,second").replace("BNB,0.09,BTC","BNB,0.09,second");
    for file in [&first,&second] {assert!(matches!(import(c,&key,"binance",file).unwrap(),Answer::Written{imported:2,..}));}
    let before=state(c);
    for file in [&second,&first] {assert!(matches!(import(c,&key,"binance",file).unwrap(),Answer::Written{imported:0,duplicates:2,..}));assert_eq!(state(c),before);}
    assert_eq!(count(c),4);
}

#[test]
fn production_csv_refuses_two_indistinguishable_pairs_without_partial_writes() {
    let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
    let first=dust_file(&["BTC"]);let second=first.replace(",BTC\n",",second\n");
    let combined=first.clone()+second.split_once('\n').unwrap().1;
    let before=state(c);assert!(matches!(import(c,&key,"binance",&combined).unwrap(),deltabadger::tracker::import::run::Answer::Refused(_)));assert_eq!(state(c),before);
    import(c,&key,"binance",&first).unwrap();let before=state(c);
    assert!(matches!(import(c,&key,"binance",&combined).unwrap(),deltabadger::tracker::import::run::Answer::Refused(_)));assert_eq!(state(c),before);
}


#[test]
fn production_csv_pair_marker_and_invalidation_share_one_commit() {
    for trigger in [
        "CREATE TEMP TRIGGER crash BEFORE INSERT ON account_transactions WHEN NEW.entry_type=2 BEGIN SELECT RAISE(ABORT,'second leg'); END",
        "CREATE TEMP TRIGGER crash BEFORE INSERT ON app_configs WHEN NEW.key='rust_binance_a12_v1:install_written' BEGIN SELECT RAISE(ABORT,'marker'); END",
        "CREATE TEMP TRIGGER crash BEFORE DELETE ON app_configs BEGIN SELECT RAISE(ABORT,'invalidation'); END",
    ] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
        let cache=deltabadger::tracker::cache::key(key.user_id);app_config::set_plain(c,&cache,"stale",now()).unwrap();
        let before=state(c);c.execute_batch(trigger).unwrap();assert!(import(c,&key,"binance",&dust_file(&["BTC"])).is_err());assert_eq!(state(c),before);
        c.execute_batch("DROP TRIGGER crash").unwrap();import(c,&key,"binance",&dust_file(&["BTC"])).unwrap();
        assert_eq!(count(c),2);assert!(writer::written(c).unwrap());assert!(app_config::get_plain(c,&cache).unwrap().is_none());
    }
}

#[test]
fn seeded_recovery_reenables_one_whole_pair_and_csv_stays_duplicate_free() {
    let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let lock=claim(c,dir.path());
    write(c,&key,&[pair("p1","BTC"),pair("p1","ETH")].concat()).unwrap();
    import(c,&key,"binance",&dust_file(&["BTC","ETH"])).unwrap();
    c.execute_batch("CREATE TEMP TABLE recovery_rows AS SELECT * FROM account_transactions WHERE base_currency='ETH' OR tx_id LIKE 'dust-a12%'").unwrap();
    suppress(c,"ETH");
    // Seed the atomic effect of the future explicit operator recovery command.
    let tx=model::immediate(c).unwrap();
    tx.execute_batch("INSERT INTO account_transactions SELECT * FROM recovery_rows").unwrap();
    let (name,text):(String,String)=tx.query_row("SELECT key,value FROM app_configs WHERE key LIKE 'rust_binance_a12_v1:bucket:%'",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    let mut bucket:Value=serde_json::from_str(&text).unwrap();
    for b in bucket["bindings"].as_array_mut().unwrap().iter_mut().filter(|b|b["suppressed"]==true) {b["suppressed"]=json!(false);b["suppression_reason"]=Value::Null;}
    tx.execute("UPDATE app_configs SET value=?1 WHERE key=?2",rusqlite::params![bucket.to_string(),name]).unwrap();tx.commit().unwrap();
    let before=state(c);import(c,&key,"binance",&dust_file(&["BTC","ETH"])).unwrap();assert_eq!(state(c),before);
    assert!(lease::hand_back(&lock,c,&common::seed::cipher(),now()).is_err());
}

fn import_browser(c: &Connection, owner: i64, app: &deltabadger::web::App) -> common::web::Browser {
    use deltabadger::web::{csrf, session};
    c.execute("UPDATE users SET confirmed_at=created_at,setup_completed=1", []).unwrap();
    let hash: String = c.query_row("SELECT encrypted_password FROM users WHERE id=?1", [owner], |r| r.get(0)).unwrap();
    let token = csrf::new_token();
    let data = session::SessionData { user: Some((owner, hash.chars().take(29).collect())), csrf: Some(token.clone()), ..Default::default() };
    common::web::Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())),
        page: Some(format!("<meta name=\"csrf-token\" content=\"{}\">", csrf::masked(&token))) }
}

#[tokio::test(flavor="current_thread")]
async fn production_export_round_trip_preserves_fractional_identity() {
    use common::web::{self, TestClock};
    use deltabadger::tracker::import::run::Answer;
    for micros in [123_000, 999_999] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
        let mut rows=pair("p1","BTC");
        for row in &mut rows {
            let at=row.entry.transacted_at.unwrap()+chrono::Duration::microseconds(micros);
            row.entry.transacted_at=Some(at);
            row.event=Event::read(if row.entry.entry_type==2 {Source::DustIn}else{Source::DustOut}, &row.entry.raw.value,
                Context {parent:Some("p1"),occurred_ms:Some((at.timestamp_micros()+500).div_euclid(1000)),..Context::default()}).unwrap();
        }
        write(c,&key,&rows).unwrap();
        let app=web::app(dir.path(),"engine-test-secret",TestClock::at("2026-08-18T00:00:00Z"));
        let mut browser=import_browser(c,s.user_id,&app);
        let exported=browser.get(&app,"/tracker/export").await;assert_eq!(exported.status,200);
        assert!(exported.body.contains("2026-08-17T10:00:00Z"));
        let before=state(c);
        assert!(matches!(import(c,&key,"deltabadger",&exported.body).unwrap(),Answer::Written{imported:0,duplicates:2,..}));
        assert_eq!(state(c),before,"round trip preserves the original fractional timestamp and evidence");
        let changed=exported.body.replace("10:00:00Z","10:00:01Z");
        assert!(!matches!(import(c,&key,"deltabadger",&changed),Ok(Answer::Written{..})));
        assert_eq!(state(c),before,"a different exported second still refuses the file");
    }
}

#[test]
fn production_dust_repeat_normalizes_both_amounts_like_storage() {
    use deltabadger::tracker::import::run::Answer;
    for (out,inn) in [("123456789.12345678","0.09"),("1","123456789.12345678"),("123456789.12345678","0.12345678912345678")] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
        let file=dust_file(&["BTC"]).replace("BTC,-1,BTC",&format!("BTC,-{out},BTC")).replace("BNB,0.09,BTC",&format!("BNB,{inn},BTC"));
        assert!(matches!(import(c,&key,"binance",&file).unwrap(),Answer::Written{imported:2,..}));
        let before=state(c);
        for _ in 0..2 {
            assert!(matches!(import(c,&key,"binance",&file).unwrap(),Answer::Written{imported:0,duplicates:2,..}));
            assert_eq!(state(c),before);
        }
        let changed=file.replace(&format!("BNB,{inn},BTC"),"BNB,0.3,BTC");
        assert!(!matches!(import(c,&key,"binance",&changed),Ok(Answer::Written{..})));
        assert_eq!(state(c),before);
    }
}

#[tokio::test(flavor="current_thread")]
async fn production_import_route_explains_identity_refusals_after_rollback() {
    use common::web::{self, Csrf, TestClock};
    for reason in ["ambiguous CSV identity", "partial CSV dust pair", "partial existing CSV dust pair",
        "CSV dust identity has no proven whole-pair match", "CSV identities are not one-to-one",
        "exported identity has no retained binding", "exported CSV identity changed", "CSV cannot exclude a suppressed event"] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
        let app=web::app(dir.path(),"engine-test-secret",TestClock::at("2026-08-18T00:00:00Z"));
        let mut browser=import_browser(c,s.user_id,&app);
        let (format,file)=match reason {
            "ambiguous CSV identity" => {
                write(c,&key,&[input(Source::Deposit,"d1","","BTC"),input(Source::Deposit,"d2","","BTC")]).unwrap();
                ("binance","User ID,UTC_Time,Account,Operation,Coin,Change,Remark\n1,2026-08-17 10:00:00,Spot,Deposit,BTC,1,\n".into())
            }
            "partial CSV dust pair" => ("binance",dust_file(&["BTC"]).lines().take(2).collect::<Vec<_>>().join("\n")),
            "partial existing CSV dust pair" => {
                import(c,&key,"binance",&dust_file(&["BTC"])).unwrap();
                c.execute("DELETE FROM account_transactions WHERE entry_type=2",[]).unwrap();
                ("binance",dust_file(&["BTC"]))
            }
            "CSV dust identity has no proven whole-pair match" => {
                import(c,&key,"binance",&dust_file(&["BTC"])).unwrap();
                ("binance",dust_file(&["BTC"]).replace("BNB,0.09","BNB,0.2"))
            }
            "CSV identities are not one-to-one" => {
                let first=dust_file(&["BTC"]);
                ("binance",first.clone()+first.replace(",BTC\n",",second\n").split_once('\n').unwrap().1)
            }
            "exported identity has no retained binding" => ("deltabadger",deltabadger::web::tracker::CSV_HEADERS.join(",")+"\n2026-08-17T10:00:00Z,deposit,BTC,1,,,,,binance,binance-a12-v1:deposit:unknown,,\n"),
            "exported CSV identity changed" => {
                write(c,&key,&pair("p1","BTC")).unwrap();
                let exported=browser.get(&app,"/tracker/export").await;assert_eq!(exported.status,200);
                ("deltabadger",exported.body.replace(",0.1,",",0.2,"))
            }
            "CSV cannot exclude a suppressed event" => {
                write(c,&key,&pair("p1","BTC")).unwrap();suppress(c,"BTC");
                ("binance",dust_file(&["BTC"]).replace("BTC,-1","BTC,-2"))
            }
            _=>unreachable!(),
        };
        // A new dust pair lands before the refusal. Its rows, marker and cache invalidation must all roll back.
        let prefix=if format=="binance" {dust_file(&["LTC"])} else {
            deltabadger::web::tracker::CSV_HEADERS.join(",")+"\n2026-08-17T10:00:00Z,swap_out,LTC,1,,,,,binance,,dust_new,\n2026-08-17T10:00:00Z,swap_in,BNB,0.09,,,,,binance,,dust_new,\n"
        };
        let file=prefix+file.split_once('\n').unwrap().1;
        app_config::set_plain(c,&deltabadger::tracker::cache::key(key.user_id),"stale",now()).unwrap();
        let before=state(c);
        let (content_type,body)=web::multipart(&[("format",format)],&[("file","history_UTC.csv",file.as_bytes())]);
        let answer=browser.send_bytes(&app,"POST","/tracker/import",Some(body),Csrf::Header,&[("content-type",&content_type)]).await;
        assert_eq!(answer.status,422,"{reason}: {}",answer.body);
        assert!(answer.body.contains("id=\"modal\""));
        // Every one of these is the file's: the user reads what to do, never our check's name (that goes to the log).
        assert!(!answer.body.contains(reason),"{reason}: {}",answer.body);
        assert!(answer.body.contains("Nothing imported. This file doesn&#39;t match your Binance history") || answer.body.contains("Nothing imported. This file doesn't match your Binance history"),"{reason}: {}",answer.body);
        assert_eq!(state(c),before,"{reason}: the whole file must roll back before validation is shown");
    }
}

#[tokio::test(flavor="current_thread")]
async fn production_import_route_keeps_unexpected_database_errors_as_errors() {
    use common::web::{self, Csrf, TestClock};
    let (dir,o,s)=common::install();let c=&o.primary;let _key=key(c,&s);let _lock=claim(c,dir.path());
    c.execute_batch("CREATE TRIGGER crash BEFORE INSERT ON account_transactions WHEN NEW.entry_type=2 BEGIN SELECT RAISE(ABORT,'unexpected database failure'); END").unwrap();
    let app=web::app(dir.path(),"engine-test-secret",TestClock::at("2026-08-18T00:00:00Z"));
    let mut browser=import_browser(c,s.user_id,&app);let before=state(c);
    let file=dust_file(&["BTC"]);
    let (content_type,body)=web::multipart(&[("format","binance")],&[("file","history_UTC.csv",file.as_bytes())]);
    let answer=browser.send_bytes(&app,"POST","/tracker/import",Some(body),Csrf::Header,&[("content-type",&content_type)]).await;
    assert_eq!(answer.status,500,"{}",answer.body);assert_eq!(state(c),before);
}

// PR 3: maintenance uses the live guarded writer, without normal-sync mutation paths.
use deltabadger::sync::binance_repair as repair;
fn evidence(rows: Vec<writer::Input>) -> repair::Evidence { repair::Evidence { inputs: rows, coverage: vec![] } }
fn plan(c: &Connection, key: &sync::Key, fetched: &repair::Evidence) -> repair::Manifest {
    let version=model::credential_version_by_id(c,key.id).unwrap().unwrap();
    repair::dry_run(c,&common::seed::cipher(),key,&version,now(),fetched).unwrap()
}
fn apply(c: &Connection,key: &sync::Key,m:&repair::Manifest,fetched:&repair::Evidence) -> Result<usize,sync::SyncError> {
    let mut inserted=0;for n in 0..m.batches.len() {inserted+=repair::apply_batch(c,&common::seed::cipher(),key,m,&m.digest(),fetched,n)?;}Ok(inserted)
}

#[test]
fn repair_dry_run_is_read_only_apply_preserves_survivors_and_retry_is_noop() {
    let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let lock=claim(c,dir.path());
    let a=input(Source::Deposit,"d1","","BTC");let b=input(Source::Deposit,"d2","","BTC");
    write(c,&key,std::slice::from_ref(&a)).unwrap();
    c.execute("DELETE FROM app_configs WHERE key LIKE 'rust_binance_a12_v1:%'",[]).unwrap();
    c.execute("UPDATE account_transactions SET manual_values='{\"price\":\"12.34\"}',transfer_link_rejected=1,api_key_id=NULL",[]).unwrap();
    let fetched=evidence(vec![b,a]);let before=state(c);let changes=c.total_changes();let m=plan(c,&key,&fetched);
    let roundtrip:repair::Manifest=serde_json::from_str(&serde_json::to_string_pretty(&m).unwrap()).unwrap();assert_eq!(roundtrip.digest(),m.digest());
    assert_eq!(state(c),before);assert_eq!(c.total_changes(),changes,"dry run never executes a write on the source connection");
    assert_eq!(m.batches.iter().map(|b|b.inserted.len()).sum::<usize>(),1);
    assert!(repair::apply_batch(c,&common::seed::cipher(),&key,&m,"wrong digest",&fetched,0).is_err());assert_eq!(state(c),before);
    assert_eq!(apply(c,&key,&m,&fetched).unwrap(),1);assert_eq!(count(c),2);
    let after=state(c);let changes=c.total_changes();assert_eq!(apply(c,&key,&m,&fetched).unwrap(),0);assert_eq!(state(c),after);assert_eq!(c.total_changes(),changes);
    assert!(lease::hand_back(&lock,c,&common::seed::cipher(),now()).is_err());
    let fresh=plan(c,&key,&fetched);assert!(fresh.batches.is_empty());assert_eq!(state(c),after);
}

#[test]
fn repair_crash_commits_whole_buckets_with_journal_and_cache_invalidation() {
    for trigger in ["CREATE TEMP TRIGGER crash BEFORE INSERT ON app_configs WHEN NEW.key LIKE 'rust_binance_a12_v1:repair:%' BEGIN SELECT RAISE(ABORT,'journal'); END",
        "CREATE TEMP TRIGGER crash BEFORE DELETE ON app_configs BEGIN SELECT RAISE(ABORT,'cache'); END",
        "CREATE TEMP TRIGGER crash BEFORE INSERT ON account_transactions WHEN NEW.entry_type=2 BEGIN SELECT RAISE(ABORT,'second leg'); END"] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
        let mut deposit=input(Source::Deposit,"d1","","BTC");deposit.entry.tx_id=Some("a-hash".into());
        let mut raw=deposit.entry.raw.value.clone();raw["txId"]=json!("a-hash");deposit.event=Event::read(Source::Deposit,&raw,Context::default()).unwrap();deposit.entry.raw=Raw::from_value(raw);
        let fetched=evidence([vec![deposit],pair("parent","ETH")].concat());
        let cache=deltabadger::tracker::cache::key(key.user_id);app_config::set_plain(c,&cache,"stale",now()).unwrap();
        c.execute("INSERT INTO portfolio_snapshots(user_id,date,created_at,updated_at) VALUES (?1,'2099-01-01','2099-01-01','2099-01-01')",[key.user_id]).unwrap();
        let m=plan(c,&key,&fetched);assert_eq!(m.batches.len(),2);
        repair::apply_batch(c,&common::seed::cipher(),&key,&m,&m.digest(),&fetched,0).unwrap();
        app_config::set_plain(c,&cache,"stale",now()).unwrap();let before=state(c);
        c.execute_batch(trigger).unwrap();assert!(repair::apply_batch(c,&common::seed::cipher(),&key,&m,&m.digest(),&fetched,1).is_err());assert_eq!(state(c),before);
        c.execute_batch("DROP TRIGGER crash").unwrap();let reopened=Connection::open(dir.path().join("production.sqlite3")).unwrap();
        assert_eq!(apply(&reopened,&key,&m,&fetched).unwrap(),2);assert_eq!(count(c),3);
        assert!(app_config::get_plain(c,&cache).unwrap().is_none());let after=state(c);assert_eq!(apply(c,&key,&m,&fetched).unwrap(),0);assert_eq!(state(c),after);
    }
}

#[test]
fn repair_refuses_changed_credentials_ownership_evidence_and_survivors() {
    for change in ["credentials","ownership","survivor","evidence","suppression"] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
        let a=input(Source::Deposit,"d1","","BTC");write(c,&key,std::slice::from_ref(&a)).unwrap();
        let mut fetched=evidence(vec![a,input(Source::Deposit,"d2","","BTC")]);let m=plan(c,&key,&fetched);
        match change {
            "credentials"=>{c.execute("UPDATE api_keys SET key='rotated' WHERE id=?1",[key.id]).unwrap();},
            "ownership"=>{c.execute("DELETE FROM app_configs WHERE key='engine_lease'",[]).unwrap();},
            "survivor"=>{c.execute("UPDATE account_transactions SET manual_values='{\"price\":\"42\"}'",[]).unwrap();},
            "evidence"=>{fetched.inputs.push(input(Source::Deposit,"d3","","BTC"));},
            "suppression"=>{suppress(c,"BTC");},_=>unreachable!()
        }
        let before=state(c);assert!(apply(c,&key,&m,&fetched).is_err(),"{change}");assert_eq!(state(c),before);
    }
}

#[test]
fn repair_reports_ambiguous_buckets_and_accounts_without_credentials() {
    let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
    let a=input(Source::Deposit,"d1","","BTC");write(c,&key,std::slice::from_ref(&a)).unwrap();
    c.execute("DELETE FROM app_configs WHERE key LIKE 'rust_binance_a12_v1:bucket:%'",[]).unwrap();
    c.execute("UPDATE account_transactions SET raw_data='{}'",[]).unwrap();let before=state(c);
    let fetched=evidence(vec![a,input(Source::Deposit,"d2","","BTC")]);let m=plan(c,&key,&fetched);
    assert_eq!(m.unresolved.len(),1);assert!(m.batches.is_empty());assert_eq!(state(c),before);
    c.execute("UPDATE account_transactions SET api_key_id=NULL",[]).unwrap();
    c.execute("DELETE FROM api_keys WHERE id=?1",[key.id]).unwrap();
    let inventory=repair::inventory(c).unwrap();assert!(inventory.iter().any(|a|a.user_id==key.user_id && a.exchange_id==key.exchange_id && a.key_id.is_none()));
}

#[derive(Clone)]
struct RepairVenue { state:Option<Value>, mode:&'static str, calls:std::rc::Rc<std::cell::RefCell<Vec<deltabadger::venue::http::HttpRequest>>> }
impl deltabadger::venue::http::Transport for RepairVenue {
    async fn send(&self,r:&deltabadger::venue::http::HttpRequest)->Result<deltabadger::venue::http::HttpResponse,deltabadger::venue::http::TransportError> {
        use sync::binance_ledger as b;
        self.calls.borrow_mut().push(r.clone());
        if ["transient","forbidden"].contains(&self.mode) {return Ok(deltabadger::venue::http::HttpResponse{status:if self.mode=="forbidden" {403}else{503},body:"secret diagnostic".into()})}
        let q=|key:&str|r.query.iter().find(|(k,_)|*k==key).unwrap().1.parse::<i64>().unwrap();
        let start=q("startTime");let end=q("endTime");
        if let Some(state)=&self.state {
            let source=if r.path==b::DEPOSITS_PATH {&state["deposits"]}else if r.path==b::WITHDRAWALS_PATH {&state["withdrawals"]}else if r.path==b::DUST_PATH {&state["dust"]}else {
                let parts:Vec<_>=r.path.split('/').collect();&state["earn"][format!("{}_{}",parts[4],parts[6])]
            };
            let rows:Vec<_>=source.as_array().into_iter().flatten().filter(|v| {
                let at=v["insertTime"].as_i64().or_else(||v["operateTime"].as_i64()).or_else(||v["time"].as_i64()).or_else(||v["applyTime"].as_str().and_then(b::apply_time).map(|t|t.timestamp_millis())).unwrap();at>=start && at<=end
            }).cloned().collect();
            let body=if [b::DEPOSITS_PATH,b::WITHDRAWALS_PATH].contains(&r.path.as_str()){json!(rows)}else if r.path==b::DUST_PATH {json!({"userAssetDribblets":rows})}else{json!({"total":rows.len(),"rows":rows})};
            return Ok(deltabadger::venue::http::HttpResponse{status:200,body:body.to_string()});
        }
        let mut raw=input(Source::Deposit,"d1","","BTC").entry.raw.value;raw["status"]=json!(1);
        let body=if r.path==b::DEPOSITS_PATH && start<=1786960800000 && end>=1786960800000 {
            match self.mode {"unavailable"=>return Ok(deltabadger::venue::http::HttpResponse{status:410,body:"secret diagnostic".into()}),
                "malformed"=>json!({}),"repeat"=>json!(vec![raw;1000]),_=>json!([raw])}
        }else if [b::DEPOSITS_PATH,b::WITHDRAWALS_PATH].contains(&r.path.as_str()){json!([])}
        else if r.path==b::DUST_PATH {if self.mode=="missing" {json!({})}else{json!({"userAssetDribblets":[]})}}
        else {if self.mode=="missing" {json!({"rows":[]})}else{json!({"total":0,"rows":[]})}};
        Ok(deltabadger::venue::http::HttpResponse{status:200,body:body.to_string()})
    }
}
#[tokio::test(flavor="current_thread")]
async fn repair_fetch_scans_full_history_and_never_completes_unavailable_windows() {
    use deltabadger::venue::binance::{BinanceVenue,URL,US_URL};use sync::binance_ledger as b;
    for (mode,us) in [("ok",false),("ok",true),("unavailable",false),("malformed",true),("repeat",true),("missing",false)] {
        let calls=std::rc::Rc::new(std::cell::RefCell::new(vec![]));let transport=RepairVenue{state:None,mode,calls:calls.clone()};
        let venue=BinanceVenue::new(transport,if us {US_URL}else{URL},"test-secret");
        let e=repair::fetch(&venue,&FixedClock(now()),now(),us,b::Pacing::NONE).await;
        assert_eq!(e.inputs.len(),if ["ok","missing"].contains(&mode){1}else{0});
        let calls=calls.borrow();assert_eq!(calls[0].query.iter().find(|(k,_)|*k=="startTime").unwrap().1,b::HISTORY_FROM_MS.to_string());
        assert!(calls.iter().all(|r|!r.path.contains("margin") && !r.path.contains("futures") && !r.path.contains("myTrades")));
        if us {assert!(calls.iter().all(|r|[b::DEPOSITS_PATH,b::WITHDRAWALS_PATH].contains(&r.path.as_str())));}
        if ["unavailable","malformed","repeat"].contains(&mode) {assert!(e.coverage.iter().any(|w|w.source==b::DEPOSITS_PATH && w.unresolved.is_some()));}
        if mode=="missing" {assert!(e.coverage.iter().filter(|w|w.source.contains("simple-earn") || w.source==b::DUST_PATH).all(|w|w.unresolved.is_some()));}
        assert!(!serde_json::to_string(&e.coverage).unwrap().contains("secret diagnostic"));
    }
}

#[test]
fn repair_metadata_only_marker_is_atomic_and_suppressions_remain_unrecovered() {
    let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
    let fetched=evidence(vec![input(Source::Deposit,"d1","","BTC")]);write(c,&key,&fetched.inputs).unwrap();
    c.execute("DELETE FROM app_configs WHERE key LIKE 'rust_binance_a12_v1:%'",[]).unwrap();let m=plan(c,&key,&fetched);
    assert_eq!(m.batches.len(),1);assert_eq!(m.tally.missing,0);
    let before=state(c);c.execute_batch("CREATE TEMP TRIGGER crash BEFORE INSERT ON app_configs WHEN NEW.key='rust_binance_a12_v1:install_written' BEGIN SELECT RAISE(ABORT,'marker'); END").unwrap();
    assert!(apply(c,&key,&m,&fetched).is_err());assert_eq!(state(c),before);c.execute_batch("DROP TRIGGER crash").unwrap();
    assert_eq!(apply(c,&key,&m,&fetched).unwrap(),0);assert!(writer::written(c).unwrap());
    suppress(c,"BTC");let before=state(c);let m=plan(c,&key,&fetched);assert_eq!(m.tally.suppressed,1);assert!(m.batches.is_empty());assert_eq!(state(c),before);
}

#[tokio::test(flavor="current_thread")]
async fn repair_matches_recorded_accounting_for_collision_free_and_lost_dust_siblings() {
    use deltabadger::venue::binance::{BinanceVenue,URL};
    let fixture:Value=serde_json::from_str(include_str!("fixtures/binance_identity.json")).unwrap();
    for (colliding,earn) in [(false,false),(false,true),(true,false),(true,true)] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let lock=claim(c,dir.path());
        let (venue_state,file)=accounting_history(&fixture,colliding,earn);import(c,&key,"deltabadger",&file).unwrap();
        let venue=BinanceVenue::new(RepairVenue{state:Some(venue_state),mode:"ok",calls:Default::default()},URL,"test-secret");
        let fetched=repair::fetch(&venue,&FixedClock(now()),now(),false,sync::binance_ledger::Pacing::NONE).await;
        write(c,&key,&fetched.inputs).unwrap();
        // Model an install with a completed normal sync. Repair must preserve this provenance,
        // not pretend its affected-source-only scan completed a full sync for a rotated key.
        let version=model::credential_version_by_id(c,key.id).unwrap().unwrap();let tx=model::immediate(c).unwrap();
        sync::cache::record_ledger(&model::fence_versions(&tx,std::slice::from_ref(&version)).unwrap(),key.id,&version,now()).unwrap();tx.commit().unwrap();
        // Reproduce Rails' retained rows: the unique legacy IDs discard the sibling pair.
        c.execute("DELETE FROM account_transactions WHERE tx_id LIKE 'dust-a12-v1-%'",[]).unwrap();
        c.execute("DELETE FROM app_configs WHERE key LIKE 'rust_binance_a12_v1:%'",[]).unwrap();
        c.execute("UPDATE account_transactions SET group_id='dust_detail' WHERE tx_id LIKE 'dust-detail-%'",[]).unwrap();
        if !colliding {accounting(c,key.user_id,&fixture["accounting"][0]);}
        let m=plan(c,&key,&fetched);assert!(m.unresolved.is_empty(),"{:?}",m.unresolved);assert_eq!(m.tally.missing,if colliding {2}else{0});
        assert_eq!(apply(c,&key,&m,&fetched).unwrap(),if colliding {2}else{0});accounting(c,key.user_id,&fixture["accounting"][0]);
        let before=state(c);assert_eq!(apply(c,&key,&m,&fetched).unwrap(),0);assert_eq!(state(c),before);
        if colliding {rebuild_from_cold(c,dir.path(),key.user_id).await;}
        let csv=dust_file(&["BTC","ETH"]).replace("2026-08-17 10:00:00","2026-08-08 00:00:00");
        import(c,&key,"binance",&csv).unwrap();accounting(c,key.user_id,&fixture["accounting"][0]);
        drop(lock);let lock=lease::lock(&store::Paths::from_env(&|_|None,dir.path()),now()).unwrap();
        let reopened=Connection::open(dir.path().join("production.sqlite3")).unwrap();
        assert!(lease::hand_back(&lock,&reopened,&common::seed::cipher(),now()).is_err());
    }
}

#[test]
fn repair_rechecks_each_batch_after_credentials_or_ownership_change() {
    for change in ["credentials","ownership","parallel import"] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
        let fetched=evidence([vec![input(Source::Deposit,"d1","","BTC")],pair("p","ETH")].concat());let m=plan(c,&key,&fetched);
        repair::apply_batch(c,&common::seed::cipher(),&key,&m,&m.digest(),&fetched,0).unwrap();
        match change {
            "credentials"=>{c.execute("UPDATE api_keys SET key='rotated'",[]).unwrap();},
            "ownership"=>{c.execute("DELETE FROM app_configs WHERE key='engine_lease'",[]).unwrap();},
            _=>{write(c,&key,&[input(Source::Deposit,"parallel","","LTC")]).unwrap();},
        }
        let before=state(c);assert!(repair::apply_batch(c,&common::seed::cipher(),&key,&m,&m.digest(),&fetched,1).is_err());assert_eq!(state(c),before);
    }
}

#[test]
fn repair_preserves_manual_inverse_and_bot_links_byte_for_byte() {
    use common::seed::{BotSpec,TxSpec,insert_bot,insert_tx};
    for inverse in [false,true] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
        let original=input(Source::Deposit,"d1","","BTC");write(c,&key,std::slice::from_ref(&original)).unwrap();
        c.execute("DELETE FROM app_configs WHERE key LIKE 'rust_binance_a12_v1:%'",[]).unwrap();
        let id:i64=c.query_row("SELECT id FROM account_transactions",[],|r|r.get(0)).unwrap();
        c.execute("INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_amount,tx_id,raw_data,transacted_at,created_at,updated_at) SELECT user_id,exchange_id,5,base_currency,base_amount,'other','{}',transacted_at,created_at,updated_at FROM account_transactions",[]).unwrap();let other=c.last_insert_rowid();
        let bot=insert_bot(c,&s,&BotSpec::weekly(60.0,"2026-09-01 10:00:00"));
        let bot_tx=insert_tx(c,&s,bot,&TxSpec{status:2,external_status:None,external_id:None,order_type:1,amount:Some("1"),quote_amount:None,price:None,quote_amount_exec:None,amount_exec:None,created_at:"2026-08-17 10:00:00".into()});
        c.execute("UPDATE account_transactions SET transaction_id=?1,manual_values='{\"price\":\"42\"}',api_key_id=NULL WHERE id=?2",params![bot_tx,id]).unwrap();
        c.execute("UPDATE account_transactions SET linked_transaction_id=?1 WHERE id=?2",if inverse {[id,other]}else{[other,id]}).unwrap();
        let before=state(c);let fetched=evidence(vec![original,input(Source::Deposit,"d2","","BTC")]);let m=plan(c,&key,&fetched);
        assert_eq!(apply(c,&key,&m,&fetched).unwrap(),1);let after=state(c);
        assert_eq!(&after[0].as_array().unwrap()[..2],before[0].as_array().unwrap());assert_eq!(after[4],before[4]);assert_eq!(after[5],before[5]);
    }
}

#[test]
fn repair_refuses_nil_twins_changed_amounts_deleted_aliases_and_partial_pairs() {
    for case in ["nil twin","changed amount","deleted alias","partial pair","missing ID"] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
        let mut fetched=evidence(if case=="partial pair" {pair("p","BTC")}else{vec![input(Source::Deposit,"d1","","BTC")]});write(c,&key,&fetched.inputs).unwrap();
        match case {
            "nil twin"=>{c.execute("DELETE FROM app_configs WHERE key LIKE 'rust_binance_a12_v1:bucket:%'",[]).unwrap();c.execute("UPDATE account_transactions SET tx_id=NULL",[]).unwrap();},
            "changed amount"=>{c.execute("UPDATE account_transactions SET raw_data=json_set(raw_data,'$.amount','2')",[]).unwrap();},
            "deleted alias"=>{c.execute("DELETE FROM account_transactions",[]).unwrap();},
            "partial pair"=>{c.execute("DELETE FROM account_transactions WHERE entry_type=2",[]).unwrap();},
            _=>{let i=&mut fetched.inputs[0];i.entry.raw.value["id"]=Value::Null;i.event=Event::read(Source::Deposit,&i.entry.raw.value,Context::default()).unwrap();},
        }
        let before=state(c);let m=plan(c,&key,&fetched);assert!(!m.unresolved.is_empty(),"{case}");assert!(m.batches.is_empty());assert_eq!(state(c),before);
    }
}

#[tokio::test(flavor="current_thread")]
async fn repair_then_restart_refuses_the_handback_command() {
    let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let lock=claim(c,dir.path());
    let fetched=evidence(pair("p","BTC"));let m=plan(c,&key,&fetched);apply(c,&key,&m,&fetched).unwrap();drop(lock);
    let before=state(c);
    // A pinned cutoff at the opening has no reachable HTTP windows. Exercise the real
    // approved apply command after restart without live credentials or network traffic.
    let cutoff=DateTime::from_timestamp_millis(sync::binance_ledger::HISTORY_FROM_MS).unwrap();
    let venue=deltabadger::venue::binance::BinanceVenue::new(RepairVenue{state:None,mode:"ok",calls:Default::default()},deltabadger::venue::binance::URL,"test-secret");
    let empty=repair::fetch(&venue,&FixedClock(now()),cutoff,false,sync::binance_ledger::Pacing::NONE).await;
    let version=model::credential_version_by_id(c,key.id).unwrap().unwrap();
    let m=repair::dry_run(c,&common::seed::cipher(),&key,&version,cutoff,&empty).unwrap();let path=dir.path().join("approved.json");std::fs::write(&path,serde_json::to_vec(&m).unwrap()).unwrap();
    let out=std::process::Command::new(env!("CARGO_BIN_EXE_deltabadger")).args(["repair-binance-history","--apply","--manifest",path.to_str().unwrap(),"--approve",&m.digest()])
        .env("STORAGE_DIR",dir.path()).env("SECRET_KEY_BASE","engine-test-secret").env_remove("DATABASE_URL").env_remove("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY").env_remove("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT").output().unwrap();
    assert!(out.status.success(),"{}",String::from_utf8_lossy(&out.stderr));assert_eq!(state(c),before);
    let out=std::process::Command::new(env!("CARGO_BIN_EXE_deltabadger")).arg("handback").env("STORAGE_DIR",dir.path()).env("SECRET_KEY_BASE","engine-test-secret")
        .env_remove("DATABASE_URL").env_remove("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY").env_remove("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT").output().unwrap();
    assert!(!out.status.success());assert!(String::from_utf8_lossy(&out.stderr).contains("Binance"),"{}",String::from_utf8_lossy(&out.stderr));assert_eq!(state(c),before);
}

#[test]
fn repair_refuses_evidence_overlapping_an_unreachable_window() {
    let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
    let mut fetched=evidence(vec![input(Source::Deposit,"d1","","BTC")]);let at=fetched.inputs[0].entry.transacted_at.unwrap().timestamp_millis();
    fetched.coverage.push(repair::Window{source:sync::binance_ledger::DEPOSITS_PATH.into(),kind:None,start:at-1000,end:at,pages:vec![],unresolved:Some("venue window unavailable".into())});
    let before=state(c);let m=plan(c,&key,&fetched);assert_eq!(m.tally.unresolved,1);assert!(m.batches.is_empty());assert_eq!(state(c),before);
}

// The update schedules the repair; these tests use the same job and guarded apply path.
impl repair::HistoryReader for RepairVenue {
    async fn read(&self, key: &sync::Key, _: &deltabadger::crypto::Credentials, _: Option<&str>, clock: &dyn deltabadger::engine::Clock, cutoff: DateTime<Utc>) -> repair::Evidence {
        let venue=deltabadger::venue::binance::BinanceVenue::new(self.clone(),deltabadger::venue::binance::URL,"test-secret");
        repair::fetch(&venue,clock,cutoff,key.exchange_type==deltabadger::venue::binance::BINANCE_US,sync::binance_ledger::Pacing::NONE).await
    }
}
fn repair_job(mode: &'static str) -> repair::HistoryRepair<RepairVenue> {
    repair::HistoryRepair::new(RepairVenue{state:None,mode,calls:Default::default()})
}
async fn run_repair(job: &impl deltabadger::jobs::Job, c: Connection) -> deltabadger::jobs::Outcome {
    job.run(deltabadger::jobs::Cx{db:deltabadger::jobs::Db::new(c,common::seed::cipher()),clock:&FixedClock(now()),wakers:Default::default()},vec![]).await
}
#[tokio::test(flavor="current_thread")]
async fn repair_update_runs_once_per_install_version_and_new_version_runs_again() {
    use deltabadger::jobs::{Outcome,state};
    for venue in [deltabadger::venue::binance::BINANCE,deltabadger::venue::binance::BINANCE_US] {
        let (dir,o,s)=common::install();let key=key(&o.primary,&s);
        o.primary.execute("UPDATE exchanges SET type=?1 WHERE id=?2",(venue,key.exchange_id)).unwrap();let _lock=claim(&o.primary,dir.path());
        let job=repair_job("ok");let calls=job.reader.calls.clone();
        assert_eq!(run_repair(&job,o.primary).await,Outcome::Done);
        let c=Connection::open(dir.path().join("production.sqlite3")).unwrap();
        assert_eq!(count(&c),1);
        let saved=state::read(&c,repair::JOB,Some(&repair::REPAIR_VERSION.to_string())).unwrap();
        assert_eq!(saved.revision,Some(repair::REPAIR_VERSION));assert!(saved.last_success_at.is_some());
        let before=state(&c);let requests=calls.borrow().len();
        assert_eq!(run_repair(&job,Connection::open(c.path().unwrap()).unwrap()).await,Outcome::NothingNew);
        assert_eq!(calls.borrow().len(),requests);assert_eq!(state(&c),before);
        let next=job.with_version(repair::REPAIR_VERSION+1);
        assert_eq!(run_repair(&next,Connection::open(c.path().unwrap()).unwrap()).await,Outcome::Done);
        assert!(calls.borrow().len()>requests);assert_eq!(count(&c),1);
        assert!(deltabadger::tracker::gaps::read(&c,key.user_id,None,None,None).unwrap().iter().any(|g|g.exchange_id==key.exchange_id),"retention/excluded windows reach the existing Import history line");
    }
}

#[tokio::test(flavor="current_thread")]
async fn repair_update_without_a_binance_key_or_rust_ownership_does_nothing() {
    use deltabadger::jobs::{Outcome,state};
    for owned in [false,true] {
        let (dir,o,s)=common::install();let lock=owned.then(||claim(&o.primary,dir.path()));
        if !owned {key(&o.primary,&s);}
        let job=repair_job("ok");let calls=job.reader.calls.clone();
        assert_eq!(run_repair(&job,o.primary).await,Outcome::NothingNew);
        let c=Connection::open(dir.path().join("production.sqlite3")).unwrap();
        assert!(calls.borrow().is_empty());assert!(state::find(&c,repair::JOB,Some(&repair::REPAIR_VERSION.to_string())).unwrap().is_none());
        drop(lock);
    }
}
#[tokio::test(flavor="current_thread")]
async fn repair_update_crash_after_committed_bucket_resumes_on_restart() {
    use deltabadger::jobs::Outcome;
    let (dir,o,s)=common::install();let c=&o.primary;key(c,&s);let _lock=claim(c,dir.path());
    let mut a=input(Source::Deposit,"d1","","BTC").entry.raw.value;a["status"]=json!(1);
    let mut b=a.clone();b["id"]=json!("d2");b["txId"]=json!("second");
    let job=repair::HistoryRepair::new(RepairVenue{state:Some(json!({"deposits":[a,b]})),mode:"ok",calls:Default::default()});
    c.execute_batch("CREATE TEMP TRIGGER repair_crash BEFORE INSERT ON account_transactions WHEN NEW.tx_id='second' BEGIN SELECT RAISE(ABORT,'crash'); END").unwrap();
    assert!(matches!(run_repair(&job,o.primary).await,Outcome::Transient(_)));
    let c=Connection::open(dir.path().join("production.sqlite3")).unwrap();assert_eq!(count(&c),1);
    assert!(c.query_row("SELECT count(*) FROM app_configs WHERE key LIKE 'rust_binance_a12_v1:repair:%'",[],|r|r.get::<_,i64>(0)).unwrap()>0);
    assert_eq!(run_repair(&job,Connection::open(c.path().unwrap()).unwrap()).await,Outcome::Done);assert_eq!(count(&c),2);
    assert_eq!(run_repair(&job,Connection::open(c.path().unwrap()).unwrap()).await,Outcome::NothingNew);assert_eq!(count(&c),2);
}
#[tokio::test(flavor="current_thread")]
async fn repair_update_unresolved_identity_is_visible_on_the_gap_line() {
    let (dir,o,s)=common::install();let key=key(&o.primary,&s);let _lock=claim(&o.primary,dir.path());
    let a=input(Source::Deposit,"d1","","BTC");write(&o.primary,&key,std::slice::from_ref(&a)).unwrap();
    o.primary.execute("DELETE FROM app_configs WHERE key LIKE 'rust_binance_a12_v1:bucket:%'",[]).unwrap();
    o.primary.execute("UPDATE account_transactions SET raw_data='{}'",[]).unwrap();
    struct Ambiguous;
    impl repair::HistoryReader for Ambiguous {
        async fn read(&self,_:&sync::Key,_:&deltabadger::crypto::Credentials,_:Option<&str>,_:&dyn deltabadger::engine::Clock,_:DateTime<Utc>)->repair::Evidence {
            evidence(vec![input(Source::Deposit,"d1","","BTC"),input(Source::Deposit,"d2","","BTC")])
        }
    }
    assert_eq!(run_repair(&repair::HistoryRepair::new(Ambiguous),o.primary).await,deltabadger::jobs::Outcome::Done);
    let c=Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let gaps=deltabadger::tracker::gaps::read(&c,key.user_id,Some(key.exchange_id),None,None).unwrap();assert_eq!(gaps.len(),1);
    assert!(deltabadger::tracker::gaps::read(&c,key.user_id+1,None,None,None).unwrap().is_empty());
    c.execute("UPDATE api_keys SET key='rotated' WHERE id=?1",[key.id]).unwrap();
    assert!(deltabadger::tracker::gaps::read(&c,key.user_id,None,None,None).unwrap().is_empty(),"the diagnostic is credential fenced");
}
#[tokio::test(flavor="current_thread")]
async fn repair_update_venue_failure_uses_scheduler_retry_without_blocking_start() {
    use deltabadger::jobs::{Job,Outcome,Retry,Scheduler};
    let (dir,o,s)=common::install();key(&o.primary,&s);let _lock=claim(&o.primary,dir.path());
    let job=repair_job("transient");let calls=job.reader.calls.clone();
    assert_eq!(job.spec().retry,Retry::Polynomial{attempts:5});
    let spec=job.spec();let scheduler=Scheduler::new(o.primary,common::seed::cipher(),vec![Box::new(job)],None);
    scheduler.wakers().wake(spec.name,spec.scope.as_deref(),None);
    assert!(calls.borrow().is_empty(),"construction performs no venue reads");
    let (stop,rx)=tokio::sync::watch::channel(false);
    let clock=deltabadger::engine::SystemClock;
    let (out,())=tokio::join!(scheduler.run(rx,&clock),async {
        tokio::time::timeout(std::time::Duration::from_secs(15),async {
            loop {if calls.borrow().len()>=2 {break} tokio::time::sleep(std::time::Duration::from_millis(20)).await;}
        }).await.unwrap();stop.send(true).unwrap();
    });out.unwrap();
    let c=Connection::open(dir.path().join("production.sqlite3")).unwrap();
    assert!(deltabadger::jobs::state::read(&c,repair::JOB,Some(&repair::REPAIR_VERSION.to_string())).unwrap().last_success_at.is_none());
    assert_eq!(run_repair(&repair_job("ok"),c).await,Outcome::Done);
}

#[test]
fn repair_session_keeps_guards_between_batches_and_avoids_repeated_full_scans() {
    for change in ["none","same connection","other connection","trigger"] {
        let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
        let fetched=evidence((0..12).map(|n|input(Source::Deposit,&format!("d{n}"),"",&format!("COIN{n}"))).collect());
        let m=plan(c,&key,&fetched);let mut session=repair::ApplySession::new(&m,&m.digest(),&fetched).unwrap();
        session.apply_batch(c,&common::seed::cipher(),&key,0).unwrap();
        match change {
            "same connection"=>{c.execute("UPDATE account_transactions SET manual_values='{}x'",[]).unwrap();},
            "other connection"=>{Connection::open(c.path().unwrap()).unwrap().execute("UPDATE account_transactions SET manual_values='{}x'",[]).unwrap();},
            "trigger"=>{c.execute_batch("CREATE TEMP TRIGGER alter_survivor AFTER INSERT ON account_transactions BEGIN UPDATE account_transactions SET transfer_link_rejected=1; END").unwrap();},_=>{}
        }
        let before=state(c);
        if change=="none" {
            for n in 1..m.batches.len(){session.apply_batch(c,&common::seed::cipher(),&key,n).unwrap();}
            session.verify(c,&common::seed::cipher(),&key).unwrap();assert_eq!(count(c),12);
            assert_eq!(session.full_checks(),2,"one initial validation and one final verification, independent of bucket count");
        } else {assert!(session.apply_batch(c,&common::seed::cipher(),&key,1).is_err());assert_eq!(state(c),before);}
    }
}

#[tokio::test(flavor="current_thread")]
async fn repair_update_unavailable_windows_finish_without_claiming_recovery() {
    let (dir,o,s)=common::install();let key=key(&o.primary,&s);let _lock=claim(&o.primary,dir.path());
    assert_eq!(run_repair(&repair_job("unavailable"),o.primary).await,deltabadger::jobs::Outcome::Done);
    let c=Connection::open(dir.path().join("production.sqlite3")).unwrap();assert_eq!(count(&c),0);
    assert_eq!(deltabadger::tracker::gaps::read(&c,key.user_id,None,None,None).unwrap().len(),1);
}

#[tokio::test(flavor="current_thread")]
async fn repair_update_authentication_failure_does_not_complete_the_version() {
    let (dir,o,s)=common::install();key(&o.primary,&s);let _lock=claim(&o.primary,dir.path());
    assert!(matches!(run_repair(&repair_job("forbidden"),o.primary).await,deltabadger::jobs::Outcome::Transient(_)));
    let c=Connection::open(dir.path().join("production.sqlite3")).unwrap();
    assert!(deltabadger::jobs::state::read(&c,repair::JOB,Some(&repair::REPAIR_VERSION.to_string())).unwrap().last_success_at.is_none());
    assert_eq!(run_repair(&repair_job("ok"),c).await,deltabadger::jobs::Outcome::Done);
}

#[test]
fn repair_session_zero_batch_completion_refuses_a_changed_account_scope() {
    let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
    let fetched=evidence(vec![]);let manifest=plan(c,&key,&fetched);assert!(manifest.batches.is_empty());
    let mut session=repair::ApplySession::new(&manifest,&manifest.digest(),&fetched).unwrap();
    c.execute("INSERT INTO exchanges(name,type,created_at,updated_at) VALUES ('Other Binance','Exchanges::BinanceUs','2026-01-01','2026-01-01')",[]).unwrap();
    c.execute("UPDATE api_keys SET exchange_id=?1 WHERE id=?2",(c.last_insert_rowid(),key.id)).unwrap();
    let before=state(c);
    assert!(session.verify(c,&common::seed::cipher(),&key).is_err());assert_eq!(state(c),before);
}

#[tokio::test(flavor="current_thread")]
async fn repair_update_install_completion_rechecks_each_accounts_scope() {
    let (dir,o,s)=common::install();let c=&o.primary;let key=key(c,&s);let _lock=claim(c,dir.path());
    c.execute("INSERT INTO exchanges(name,type,created_at,updated_at) VALUES ('Other Binance','Exchanges::BinanceUs','2026-01-01','2026-01-01')",[]).unwrap();
    let exchange=c.last_insert_rowid();
    c.execute_batch(&format!("CREATE TEMP TRIGGER move_scope AFTER INSERT ON app_configs WHEN NEW.key LIKE 'rust_job.binance_history_repair_account:%' BEGIN UPDATE api_keys SET exchange_id={exchange} WHERE id={}; END",key.id)).unwrap();
    let job=repair::HistoryRepair::new(RepairVenue{state:Some(json!({})),mode:"ok",calls:Default::default()});
    assert!(matches!(run_repair(&job,o.primary).await,deltabadger::jobs::Outcome::Transient(_)));
    let c=Connection::open(dir.path().join("production.sqlite3")).unwrap();
    assert!(deltabadger::jobs::state::read(&c,repair::JOB,Some(&repair::REPAIR_VERSION.to_string())).unwrap().last_success_at.is_none());
    assert_eq!(run_repair(&job,c).await,deltabadger::jobs::Outcome::Done);
}
