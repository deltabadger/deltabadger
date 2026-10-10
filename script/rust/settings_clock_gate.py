#!/usr/bin/env python3
"""Changed/new test scopes cannot mix wall time and fixed dates.
R8 also checks the production Alpaca -> HTTP absolute-send boundary: a test
can inject a fixed clock without mentioning wall time while the adapter compares
its deadline with Utc::now. No unchanged-source exemption applies there.
The adapter and LiveFactory carry the same injected clock; SystemClock is the
production default. Reqwest still enforces real elapsed-time request timeouts.
"""
from pathlib import Path
import re,json,sys,hashlib
ROOT=Path(__file__).resolve().parents[2]
WALL=re.compile(r'\b(?:(?:chrono::)?Utc|(?:std::time::)?SystemTime)::now\s*\(|\bTime\.(?:current|now)\b|\bDate\.today\b')
DATE=re.compile(r'\b\d{4}-\d{2}-\d{2}(?:[T ][0-9:]+)?\b|include_str!\s*\(\s*"[^"]*(?:wait|deadline)[^"]*"')
def scopes(text):
    # All Rust functions (tests and helper scopes), and Ruby methods/test blocks.
    starts=list(re.finditer(r'(?m)^\s*(?:(?:pub(?:\([^\n]*\))? )?(?:async )?fn \w+\(|def [^\n]+|test\s+[\'"].*? do\b)',text))
    for i,m in enumerate(starts):
        body=text[m.start():starts[i+1].start() if i+1<len(starts) else len(text)]
        if m[0].lstrip().startswith('def ') and re.search(r'(?<![=!<>])=(?!=)',m[0]):body=body.split('\n',1)[0]
        body=re.sub(r'(?m)^\s*(?://|#)[^\n]*','',body)
        yield m[0].strip(),body,m.start()
def violations(text,known=()):
    return [(name,text[:offset].count('\n')+1) for name,body,offset in scopes(text)
            if hashlib.sha256(body.encode()).hexdigest() not in known and WALL.search(body) and DATE.search(body)]
def production_scope_violations(text):
    # Scan every production Rust function, including unchanged scopes: an absolute
    # bound derived from a test clock must never enter a real-time comparison.
    return [name for name,body,offset in scopes(text)
            if WALL.search(body) and re.search(r'\b(?:not_after|deadline|send_deadline)\b',body)]

# R8 checks the production boundary even if tests never mention Utc::now.
def production_violations(http,alpaca):
    findings=[]
    for name,body,offset in scopes(http):
        if 'not_after' in body and WALL.search(body):
            findings.append(f'venue/http.rs:{name}: injected deadline reaches a wall clock')
    if 'match (not_after - self.clock.now())' not in http:
        findings.append('venue/http.rs: send bound must compare the injected clock immediately before send')
    if 'clock: std::sync::Arc<dyn crate::engine::Clock + Send + Sync>' not in http or 'self.clock = clock' not in http:
        findings.append('venue/http.rs: missing shared injected clock')
    if 'transport.with_clock(self.clock.clone())' not in alpaca:
        findings.append('venue/alpaca.rs: LiveFactory must carry its clock into HTTP')
    if 'pub fn with_clock' not in alpaca or 'self.clock=clock' not in alpaca:
        findings.append('venue/alpaca.rs: factory clock must be injectable')
    return findings

def boundary_self_test():
    for bad in ['fn send(deadline:At){if deadline < Utc::now(){return;}}','fn send(not_after:At){let now=chrono::Utc::now();let left=not_after-now;}','fn send(deadline:At){let now=SystemTime::now();assert!(deadline>now);}']:
        assert production_scope_violations(bad),bad
    assert not production_scope_violations('fn send(deadline:At){let now=clock.now();let left=deadline-now;}')
    print('PASS: R8 production scope guard rejects three real-time/absolute-bound probes; injected-clock control accepted')
    http=(ROOT/'rust/src/venue/http.rs').read_text();alpaca=(ROOT/'rust/src/venue/alpaca.rs').read_text()
    assert not production_violations(http,alpaca)
    for bad_http,bad_alpaca in [
        (http.replace('not_after - self.clock.now()','not_after - Utc::now()'),alpaca),
        (http.replace('not_after - self.clock.now()','not_after - chrono::Utc::now()'),alpaca),
        (http.replace('not_after - self.clock.now()','not_after - std::time::SystemTime::now()'),alpaca),
        (http,alpaca.replace('transport.with_clock(self.clock.clone())','transport')),
    ]:
        assert production_violations(bad_http,bad_alpaca)
    print('PASS: R8 production clock guard rejects four indirect deadline/factory bypasses; shared clock control accepted')

def check():
    base=json.loads((ROOT/'script/rust/settings_clock_baseline.json').read_text())
    findings=[];scanned=0;changed=0
    for pattern in ['rust/tests/**/*.rs','script/rust/*.rb','test/**/*.rb']:
        for path in sorted(ROOT.glob(pattern)):
            name=str(path.relative_to(ROOT));text=path.read_text();digest=hashlib.sha256(text.encode()).hexdigest();scanned+=1
            entry=base.get(name,{})
            if entry.get('sha256')==digest:continue
            changed+=1
            findings += [f'{name}:{line}: {scope}: wall clock with literal date/deadline fixture' for scope,line in violations(text,entry.get('scopes',[]))]
    production=list(sorted(ROOT.glob('rust/src/**/*.rs')))
    for path in production:
        findings += [f'{path.relative_to(ROOT)}:{scope}: absolute bound reaches production wall time' for scope in production_scope_violations(path.read_text())]
    print(f'R8 clock guard scanned all {len(production)} production Rust files for indirect real-time bounds')
    findings += production_violations((ROOT/'rust/src/venue/http.rs').read_text(),(ROOT/'rust/src/venue/alpaca.rs').read_text())
    if findings:raise SystemExit('\n'.join(findings))
    print(f'PASS: R7 clock grep gate scanned {scanned} test/oracle files; {changed} added/changed files contain no new wall-clock/fixed-date scope')
if '--self-test' in sys.argv:
    boundary_self_test()
    bad=[
        'fn expiry(){let due=chrono::Utc::now();let a="2026-10-15T12:00:29Z";assert!(due<a);}',
        'async fn expiry(){let due=chrono::Utc::now();let row=include_str!("fixtures/s1_legacy_failure_wait.json");assert!(row>due);}',
        'fn expiry(){let now=SystemTime::now();let end="2026-10-15";assert!(now<end);}',
        'def expiry\n now = Time.current\n deadline = "2026-10-15"\n assert now < deadline\nend',
        'test "expiry" do\n now = Time.now\n assert now < "2026-10-15"\nend',
    ]
    for text in bad:assert violations(text),text
    good=['fn stable(){let due=fixed_clock.now();let end="2026-10-15";assert!(due<end);}',
          'fn relative(){let due=Utc::now();let end=due+Duration::days(7);assert!(due<end);}',
          'def fixed\n now = injected_clock.now\n assert now < "2026-10-15"\nend']
    for text in good:assert not violations(text),text
    print('PASS: R7 clock grep gate rejects five expiry probes and accepts three injected/relative-clock controls')
check()
