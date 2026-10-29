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
    let tables=["account_transactions","app_configs","portfolio_snapshots","portfolio_venue_snapshots"];
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
    let tx=c.unchecked_transaction().unwrap();
    for (key,text) in buckets {let mut v:Value=serde_json::from_str(&text).unwrap();for b in v["bindings"].as_array_mut().unwrap() {
        if b["raw"]["fromAsset"]==coin || b["raw"]["coin"]==coin {
            tx.execute("DELETE FROM account_transactions WHERE id=?1",[b["row_id"].as_i64().unwrap()]).unwrap();b["suppressed"]=json!(true);b["suppression_reason"]=json!("seeded operator rollback");
        }
    } app_config::set_plain(&tx,&key,&v.to_string(),now()).unwrap();}tx.commit().unwrap();
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
