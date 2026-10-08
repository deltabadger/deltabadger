"""Check every new production function for raw fills, panics and swallowed writes."""
from pathlib import Path
import re
import subprocess
import sys
root = Path(__file__).resolve().parents[2]
def check():
    cache=(root/'rust/src/tracker/cache.rs').read_text().split('#[cfg(test)]')[0]
    producer=(root/'rust/src/tracker/jobs.rs').read_text().split('async fn cached_ledger_run',1)[1].split('/// `portfolio_backfill`',1)[0]
    jobs=(root/'rust/src/jobs/mod.rs').read_text()
    deadline=jobs.split('pub async fn settled',1)[1].split('pub fn longest_write_hold',1)[0]
    manual=(root/'rust/src/sync/jobs.rs').read_text().split('pub async fn run_within_deadline',1)[1].split('/// `ledger_sync`',1)[0]
    for code in [cache,producer,deadline,manual]:
        assert not re.search(r'\.unwrap\s*\(|\.expect\s*\(|\blet\s+_\s*=|\bpanic!|\b(?:amount_exec|quote_amount_exec)\b|\.raw\.',code), 'unchecked/discarded data or raw fill'
    assert not re.search(r'\.ok\s*\(|\.unwrap_or\s*\(',producer), 'discarded producer error'
    print('PASS new cache, producer and deadline boundaries')
if '--sensitivity' in sys.argv:
    path=root/'rust/src/tracker/cache.rs';before=path.read_text()
    try:
        for probe in ['let _ = c.execute("UPDATE users", []);','value.unwrap();','let sql = "SELECT amount_exec FROM transactions";']:
            path.write_text(before.replace('#[cfg(test)]',probe+'\n#[cfg(test)]',1))
            result=subprocess.run([sys.executable,__file__],capture_output=True,text=True)
            assert result.returncode != 0 and 'unchecked/discarded' in result.stderr
    finally:
        path.write_text(before)
check()
