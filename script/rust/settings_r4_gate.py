#!/usr/bin/env python3
"""R4: digest reads only in constructors/current_for; values retain immutable handle producers."""
from pathlib import Path
import json, re, sys, subprocess, os
root=Path(__file__).resolve().parents[2]
# This is the sole provenance exemption, naming both ends of the existing market/date store.
CONTRACT=json.loads((root/'script/rust/settings_r4_contract.json').read_text())
EXEMPTIONS=CONTRACT['exemptions']
CONSTRUCTORS={(row['file'],row['function']):row['builds'] for row in CONTRACT['constructors']}
READS=re.compile(r'\b(?:credential_version_by_id|credential_version|ciphertext_version)\s*\(')
def clean(s):
    # Keep byte positions/newlines; remove string/comment contents from executable-code searches.
    pattern=r'//[^\n]*|/\*[\s\S]*?\*/|r(#+)?"[\s\S]*?"\1|"(?:\\.|[^"\\])*"'
    return re.sub(pattern,lambda m:''.join('\n' if c=='\n' else ' ' for c in m[0]),s)
def failures():
    errors=[]
    assert len(EXEMPTIONS)==1 and EXEMPTIONS[0]['reason']=='market data, not account state; RULING-R4A'

    for p in (root/'rust/src').rglob('*.rs'):
        name=str(p.relative_to(root/'rust/src'));s=p.read_text().split('\n#[cfg(test)]')[0];code=clean(s)
        functions=list(re.finditer(r'\bfn\s+(\w+)\s*(?:<[^\n]*>)?\s*\(',code))
        for m in READS.finditer(code):
            if any(f.start()<=m.start()<f.end() for f in functions):continue
            prior=[f for f in functions if f.start()<m.start()]
            fn=prior[-1][1] if prior else ''
            if (name,fn)==('engine/model.rs',CONTRACT['comparison']['function']):continue
            if (name,fn) not in CONSTRUCTORS:errors.append(f'{name}:{fn}: digest read outside handle construction/current_for')
            elif '.await' in code[prior[-1].end():m.start()]:errors.append(f'{name}:{fn}: producer read after I/O')
        for m in re.finditer(r'\bReader::new\([^;]+',code):
            if '.with_current(' not in m[0]:errors.append(name+': production account Reader is not transaction bound')
        if name!='web/settings/validator.rs' and 'validator::check(' in code:errors.append(name+': raw credential validation bypasses immutable handle')
        if name!='engine/model.rs' and re.search(r'CredentialVersion\s*\{\s*(?:id|digest):',code):errors.append(name+': forged digest')
        if re.search(r'\bdigest\s*==|==\s*[^;\n]*\.digest\b',code):errors.append(name+': parallel digest comparison')
    for name in CONTRACT['wait_consumers']:
        code=clean((root/'rust/src'/name).read_text().split('\n#[cfg(test)]')[0])
        if re.search(r'else\s*\{\s*(?:return\s+)?(?:Ok\(\s*)?true',code) or re.search(r'_\s*=>\s*(?:Ok\(\s*)?true',code):
            errors.append(name+': permissive unknown wait provenance default')
    required=CONTRACT['required']
    for name,anchors in required.items():
        s=(root/('rust/'+name if name=='build.rs' else 'rust/src/'+name)).read_text()
        for anchor in anchors:
            if anchor not in s:errors.append(f'{name}: missing structural provenance contract: {anchor}')
    run=(root/'rust/src/engine/run.rs').read_text();key=run.split('fn market_wait_key',1)[1].split('\n}',1)[0]
    if re.search(r'credential|digest',clean(key)):errors.append('market_wait_key must never re-read producer')
    # No raw private calls in attributed engine consumers. Concrete transports keep their wire protocol.
    for name in ['tick','polling','index','splits','placement']:
        s=clean((root/f'rust/src/engine/{name}.rs').read_text().split('\n#[cfg(test)]')[0])
        if re.search(r'\bprices\.put\(',s):errors.append(name+': raw cached price bypasses Produced')
        if re.search(r'\bvenue\.(?:price|clock|balance|positions|orders_identified|fills_from_trades|add_order|order_by_client_id)\(',s):errors.append(name+': raw venue result bypasses handle envelope')
    return errors
if '--self-test' in sys.argv:
    probes=[('engine/run.rs','    closed_until: HashMap', 'fn r4_illegal_digest(c:&rusqlite::Connection){ let _=model::credential_version_by_id(c,1); }\n    closed_until: HashMap'),
            ('engine/run.rs','model::Produced::new(until.timestamp_micros(),producer)','model::Produced::new(until.timestamp_micros(),model::credential_version_by_id(&e.primary,1)?)'),
            ('figures/page_market.rs','credential_is_current(c,origin)','r4_bypass_current_for(c,origin)'),
            ('venue.rs','impl<V:Venue> Attributed for Handle<V> {}','impl<V:Venue> Attributed for V {}'),
            ('web/settings/keys.rs','handle.check(&app.settings_key_url,kind)','super::validator::check(&app.settings_key_url,handle.credentials(),kind)')]
    probes += [('engine/model.rs','if !self.complete','if false'),
               ('sync/cache.rs','.with_completion(complete)','.with_completion(true)'),
               ('engine/run.rs','if e.attempts.get(&id).is_some_and(|state|!state.current_for(&credential_version).is_fresh())','if false'),
               ('engine/run.rs','if e.retry_at.get(&id).is_some_and(|state|!state.current_for(&credential_version).is_fresh())','if false'),
               ('tracker/cache.rs','if !crate::engine::model::stamp_set_current_for(&v["producers"],&current.producers)','if false')]
    probes += [('engine/run.rs','let fresh=model::wait_is_current(&wait_tx,&wait_bot.transient["rust_defer_until"])?;','let fresh=if false {false} else { true };'),
               ('engine/model.rs','_=>Ok(false),','_=>Ok(true),'),
               ('engine/placement.rs','"origin": "local"','"legacy": "local"')]
    for name,old,new in probes:
        p=root/'rust/src'/name;green=p.read_bytes()
        try:
            assert p.read_text().count(old)>=1;p.write_text(p.read_text().replace(old,new,1));assert failures(),name
        finally:p.write_bytes(green)
    print('PASS: all 13 R4/R5/R6 structural bypass probes rejected; exact source restored')
if '--build-probes' in sys.argv:
    assert 'S1_R4_MUTATION' not in os.environ
    p=root/'rust/src/engine/run.rs';green=p.read_bytes()
    probes=[('constructor restriction',green.decode()+'\nfn r4_illegal_digest(c:&rusqlite::Connection){let _=model::credential_version_by_id(c,1); }\n'),
            ('post-call label',green.decode().replace('model::Produced::new(until.timestamp_micros(),producer)','model::Produced::new(until.timestamp_micros(),model::credential_version_by_id(&e.primary,1)?)',1)),
            ('unknown wait default',green.decode().replace('let fresh=model::wait_is_current(&wait_tx,&wait_bot.transient["rust_defer_until"])?;','let fresh=if false {false} else { true };',1))]
    try:
        for index,(name,body) in enumerate(probes):
            free=os.statvfs('/data');assert free.f_bavail*free.f_frsize>=20*1024**3
            subprocess.run(['df','-h','/data'],check=True)
            p.write_text(body)
            result=subprocess.run(['cargo','check','--manifest-path','rust/Cargo.toml','--locked','--lib'],cwd=root,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
            (Path(os.environ['S1_EVIDENCE'])/f'r4-build-rejection-{index}.log').write_text(result.stdout)
            assert result.returncode==101 and 'R4 provenance gate: engine/run.rs:' in result.stdout and ('permissive unknown wait provenance default' if index==2 else 'digest read outside handle construction/current_for') in result.stdout,(name,result.stdout[-4000:])
            p.write_bytes(green)
        validator=root/'rust/src/web/settings/keys.rs';validator_green=validator.read_bytes()
        try:
            raw=validator_green.decode().replace('handle.check(&app.settings_key_url,kind)','super::validator::check(&app.settings_key_url,handle.credentials(),kind)',1)
            assert raw!=validator_green.decode();validator.write_text(raw)
            free=os.statvfs('/data');assert free.f_bavail*free.f_frsize>=20*1024**3
            subprocess.run(['df','-h','/data'],check=True)
            result=subprocess.run(['cargo','check','--manifest-path','rust/Cargo.toml','--locked','--lib'],cwd=root,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
            (Path(os.environ['S1_EVIDENCE'])/'r4-build-rejection-3.log').write_text(result.stdout)
            assert result.returncode==101 and 'R4 provenance gate: web/settings/keys.rs: raw credential validation bypasses immutable handle' in result.stdout,result.stdout[-4000:]
        finally:validator.write_bytes(validator_green)
    finally:p.write_bytes(green)
    subprocess.run(['df','-h','/data'],check=True)
    subprocess.run(['cargo','check','--manifest-path','rust/Cargo.toml','--locked','--lib'],cwd=root,check=True)
    print('PASS: all 4 R4/R6 Cargo build rejection probes failed at provenance gate; exact source restored and unflagged build passes')
if '--types' in sys.argv:
    p=root/'rust/tests/r4_type_rejection.rs';assert not p.exists()
    try:
        free=os.statvfs('/data');assert free.f_bavail*free.f_frsize>=20*1024**3
        p.write_text('''use deltabadger::{venue::{Attributed,fake::FakeVenue},engine::tick::PriceCache,ruby::BigDec};
async fn raw_result_cannot_claim_a_handle(v:&FakeVenue,c:&PriceCache){let _=v.clock_result().await;c.put_result((1,1,deltabadger::venue::PriceSide::Ask),chrono::Utc::now(),BigDec::one());}
''')
        r=subprocess.run(['cargo','check','--manifest-path','rust/Cargo.toml','--locked','--test','r4_type_rejection'],cwd=root,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
        (Path(os.environ['S1_EVIDENCE'])/'r4-type-rejection.log').write_text(r.stdout)
        assert r.returncode==101 and 'error[E0599]' in r.stdout and 'error[E0308]' in r.stdout and 'Produced<BigDec>' in r.stdout,r.stdout[-4000:]
    finally:p.unlink(missing_ok=True)
    print('PASS: R4 raw venue and unbound cached-price type probes rejected (E0599/E0308)')
errors=failures()
if errors:raise SystemExit('\n'.join(errors))
print('PASS: R4 handle/current_for grep gate; sole exemption: '+json.dumps(EXEMPTIONS))
