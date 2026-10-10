#!/usr/bin/env python3
"""R1: Ruby validation and non-forgeable writer interfaces; negative type proof restores bytes."""
import os
from pathlib import Path
import re
import subprocess
import sys

root=Path(__file__).resolve().parents[2]

def whitespace_errors():
    errors=[]
    for path in (root/'rust/src/web/settings').glob('*.rs'):
        source=path.read_text()
        if re.search(r'(?:Regex|RegexBuilder)::new',source):
            errors.append(str(path)+': validation regex bypasses shared Ruby helper')
        if re.search(r'\.trim\(\)',source):
            errors.append(str(path)+': validation trim bypasses shared Ruby helper')
    helper=(root/'rust/src/ruby.rs').read_text()
    if 'pattern.replace(r"\\s", r"[\\x09-\\x0d\\x20]")' not in helper:
        errors.append('Ruby validation regex must give ASCII semantics to \\s')
    return errors

if '--self-test' in sys.argv:
    path=root/'rust/src/web/settings/account.rs';green=path.read_bytes()
    try:
        path.write_bytes(green.replace(b'crate::ruby::validation_regex(pattern)',b'regex::Regex::new(pattern)',1))
        assert whitespace_errors(), 'Unicode validation bypass survived'
        print('PASS: Unicode whitespace gate probe rejected')
    finally:path.write_bytes(green)
assert not whitespace_errors(),whitespace_errors()

interfaces={
 'tracker/cache.rs':['publish'],
 'tracker/snapshot.rs':['upsert_whole','upsert_venue','write'],
 'tracker/backfill.rs':['store'],
 'tracker/prices.rs':['store_bars'],
 'sync/balances.rs':['upsert'],
 'sync/ledger.rs':['save_import','store','set_assets'],
 'sync/cache.rs':['record','record_ledger'],
 'sync/mod.rs':['record_sync_error'],
 'web/settings/keys.rs':['store_status'],
 'engine/model.rs':['record_failure_origin'],
 'engine/amount.rs':['write_order_row'],
 'engine/polling.rs':['apply_in'],
 'engine/tick.rs':['stamp_funds_low','record_failure','record_notified_failure','handle_failure_inner','record_skipped','park'],
}
for filename,names in interfaces.items():
    source=(root/'rust/src'/filename).read_text()
    for name in names:
        declaration=re.search(r'fn '+name+r'\([^\n]*',source)
        assert declaration and 'FencedTransaction' in declaration[0],(filename,name)
model=(root/'rust/src/engine/model.rs').read_text()
assert "pub struct FencedTransaction<'a> { connection: &'a Connection," in model
assert "pub connection" not in model
print('PASS: all 23 venue-derived writer interfaces require the checked transaction type')

if '--types' in sys.argv:
    probe=root/'rust/tests/r1_type_rejection.rs'
    assert not probe.exists()
    text='''use deltabadger::{engine::{model,polling,amount},tracker::{snapshot,prices,backfill},web::settings::keys,sync};
fn connection_is_not_a_fenced_writer(c:&rusqlite::Connection,bot:&model::Bot,plan:&amount::OrderPlan,state:&deltabadger::venue::OrderState,day:&snapshot::Day,rows:&[prices::PriceRow],cipher:&deltabadger::crypto::Cipher,swept:&backfill::Swept,version:&model::CredentialVersion,credentials:&deltabadger::crypto::Credentials){
    let now=chrono::Utc::now();let date=now.date_naive();
    snapshot::upsert_whole(c,1,date,day).unwrap();
    snapshot::upsert_venue(c,1,1,date,day).unwrap();
    snapshot::write(c,1,&[],date).unwrap();
    keys::store_status(c,1,1,now).unwrap();
    amount::write_order_row(c,bot,plan,amount::RowKind::Skipped,now).unwrap();
    polling::apply_in(c,1,1,state,true,now).unwrap();
    prices::store_bars(c,rows).unwrap();
    backfill::store(c,cipher,1,swept,date,"v",now).unwrap();
    sync::cache::record(c,1,1,version,true,now).unwrap();
    sync::cache::record_ledger(c,1,version,now).unwrap();
    sync::record_sync_error(c,1,"fixture",credentials).unwrap();
    deltabadger::tracker::cache::publish(c,1,&deltabadger::tracker::cache::version(c,1).unwrap(),None,1,now).unwrap();
}
'''
    try:
        free=os.statvfs('/data');assert free.f_bavail*free.f_frsize>=20*1024**3
        probe.write_text(text)
        result=subprocess.run(['cargo','check','--manifest-path','rust/Cargo.toml','--locked','--test','r1_type_rejection'],cwd=root,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
        output=result.stdout
        Path(os.environ['S1_EVIDENCE']).mkdir(parents=True,exist_ok=True)
        (Path(os.environ['S1_EVIDENCE'])/'r1-type-rejection.log').write_text(output)
        assert result.returncode==101 and output.count('error[E0308]')==12,output[-5000:]
        assert output.count("expected `&FencedTransaction<'_>`, found `&Connection`")==12,output[-5000:]
        print('PASS: all 12 raw-connection writer probes rejected by Rust type checking (expected E0308)')
    finally:
        probe.unlink(missing_ok=True)
print('PASS: R1 gates; exact source restored')
