"""Assertion-only mutations, bounded jobs, and unconditional source restoration."""
from pathlib import Path
import os,subprocess,shutil,json,signal
root=Path(__file__).resolve().parents[2]
out=Path(os.environ['D5_HOLDINGS_MUTATIONS']);out.mkdir(parents=True,exist_ok=True)
paths={k:root/v for k,v in {'cache':'rust/src/tracker/cache.rs','figures':'rust/src/tracker/figures/page.rs','page':'rust/src/web/tracker/holdings.rs','card':'rust/templates/tracker/holdings.html'}.items()}
original={k:p.read_text() for k,p in paths.items()};results=[]
def interrupted(signum,frame):raise KeyboardInterrupt(signum)
signal.signal(signal.SIGTERM,interrupted)
def run(name,selection,cases=''):
    subprocess.run(['df','-g','/'],check=True);assert shutil.disk_usage('/').free>=15*1024**3,'STOP under15GiB'
    args=['cargo','nextest','run','--manifest-path','rust/Cargo.toml','--locked','-j','5','--lib','--test','tracker_holdings','--test','tracker_holdings_contract','-E',selection]
    with (out/f'{name}.log').open('w') as log:code=subprocess.run(args,cwd=root,env=dict(os.environ,D5_HOLDINGS_CASES=cases),stdout=log,stderr=subprocess.STDOUT).returncode
    return code,(out/f'{name}.log').read_text()
unit='test(cold_) | test(cache_publish_decimal)'
page='test(cold_holdings_match_rails)'
mutants=[
('nullable_symbol','figures','r.get::<_,Option<String>>(1)?.unwrap_or_default()','r.get::<_,String>(1)?','test(cold_nullable)',''),
('nullable_name','figures','r.get::<_,Option<String>>(2)?.unwrap_or_default()','r.get::<_,String>(2)?','test(cold_nullable)',''),
('publish_limit','cache','if decimal.to_s_f().len()>MAX_COMPUTED_DECIMAL','if false && decimal.to_s_f().len()>MAX_COMPUTED_DECIMAL','test(cache_publish_decimal_limit_keeps)',''),
('publish_reason','cache','crate::engine::log("[tracker] ledger cache publish refused: computed decimal exceeds 1048576-byte limit");','','test(cache_publish_decimal_limit_logs)',''),
('publish_preserve','cache','    if let Some(w)=walked {','    c.execute("DELETE FROM app_configs WHERE key=?1",[key(owner)])?;\n    if let Some(w)=walked {','test(cache_publish_decimal_limit_keeps)',''),
('owner','figures','WHERE b.user_id=?1 AND','WHERE (?1 IS NOT NULL) AND','test(cold_figures_owner)',''),
('scope','figures','(?2 IS NULL OR b.exchange_id=?2)','(?2 IS NULL OR b.exchange_id=b.exchange_id)','test(cold_figures_owner)',''),
('nonzero','figures','AND b.free+b.locked>0','AND 1=1','test(cold_sql_predicate)',''),
('asset_identity','figures','h.asset==id','h.symbol==symbol','test(cold_figures_owner)',''),
('stale','figures','}else if stale{','}else if false && stale{','test(cold_missing_or_stale)',''),
('unsynced','figures','let reason=if unsynced{','let reason=if false && unsynced{','test(cold_figures_need)',''),
('unsynced_page','figures','let reason=if unsynced{','let reason=if false && unsynced{',page,'holdings_unsynced'),
('stale_boundary','figures','>chrono::Duration::seconds(300)','>=chrono::Duration::seconds(300)','test(cold_missing_or_stale)',''),
('missing_time','figures','else if missing_time{','else if false && missing_time{','test(cold_missing_or_stale)',''),
('unpriced','figures','else if !unpriced.is_empty(){','else if false && !unpriced.is_empty(){','test(cold_missing_or_stale)',''),
('cash','figures','if !show_cash{','if false && !show_cash{','test(cold_figures_owner)',''),
('transaction_boundary','page','AND NOT EXISTS(SELECT 1 FROM account_transactions WHERE user_id=?1)','AND 1=1','test(cold_page_boundary)',''),
('history_boundary','page','AND NOT EXISTS(SELECT 1 FROM portfolio_snapshots WHERE user_id=?1)','AND 1=1','test(cold_page_boundary)',''),
('venue_history_boundary','page','AND NOT EXISTS(SELECT 1 FROM portfolio_venue_snapshots WHERE user_id=?1)','AND 1=1','test(cold_page_boundary)',''),
('cache_boundary','page','AND NOT EXISTS(SELECT 1 FROM app_configs WHERE key=?2)','AND (?2 IS NOT NULL)','test(cold_page_boundary)',''),
('key_status','page','COALESCE(k.status,-1)!=1','0','test(cold_page_boundary)',''),
('fx','page','d*&denomination.rate','d*&Dec::one()',page,'holdings_currency_EUR'),
('fx_unavailable','page','data.reason.or(if d.is_none(){Some("Currency conversion unavailable")}else{None})','data.reason',page,'holdings_fx_unavailable'),
('fold_six','page','if rows.len()>6{rows.split_off(6)}','if rows.len()>5{rows.split_off(5)}',page,'holdings_en_frame'),
('ring_tail','page','if small.len()==1{','if !small.is_empty(){',page,'holdings_en_frame'),
('type_share','page','rounded.sort_by(|a,b|b.1.cmp(&a.1))','rounded.sort_by(|a,b|a.1.cmp(&b.1))',page,'holdings_en_frame'),
('sync_age','page','else if seconds>=604800','else if false && seconds>=604800',page,'holdings_age_604800'),
('hidden_value','card','{% if hidden %}tracker-holdings--anon{% endif %}','tracker-holdings--anon',page,'holdings_en_frame'),
]
try:
    assert run('baseline',unit,'holdings_en_frame')[0]==0
    for name,key,before,after,selection,cases in mutants:
        assert original[key].count(before)==1,(name,original[key].count(before))
        try:
            paths[key].write_text(original[key].replace(before,after))
            code,log=run(name,selection,cases)
            killed=code==100 and 'test run failed' in log and 'panicked at' in log
            results.append(dict(name=name,exit=code,killed=killed));(out/'results.json').write_text(json.dumps(results,indent=2)+'\n')
            assert killed,name+' survived or did not fail assertion'
            print(name,'killed',flush=True)
        finally:paths[key].write_text(original[key])
    assert run('restored',unit,'holdings_en_frame')[0]==0
finally:
    for k,text in original.items():paths[k].write_text(text)
print(len(results),'holdings mutations killed; restored baseline green')
