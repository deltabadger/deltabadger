"""New holdings code never bypasses shared figures or discards a write error."""
from pathlib import Path
import re,subprocess,sys
root=Path(__file__).resolve().parents[2]
files=['rust/src/tracker/figures/page.rs','rust/src/web/tracker/holdings.rs']
def check():
    for name in files:
        text=(root/name).read_text().split('#[cfg(test)]')[0]
        assert not re.search(r'\.unwrap\s*\(|\.expect\s*\(|\bpanic!|\blet\s+_\s*=|\b(?:amount_exec|quote_amount_exec)\b|\.raw\.',text),'unchecked data, discarded error or raw fill: '+name
        assert not re.search(r'\.execute|wake_job|perform_later',text),'holdings read has an effect'
    index=(root/'rust/src/web/tracker/index.rs').read_text()
    assert 'super::read::only(c,|c|render(c,&ctx,owner,fx.as_ref()))' in index,'GET render must use query_only'
    print('PASS holdings boundaries')
if '--sensitivity' in sys.argv:
    path=root/files[0];original=path.read_text()
    try:
        for probe in ['value.unwrap();','let _ = persist();','SELECT amount_exec FROM transactions','wake_job();']:
            path.write_text(original+'\n'+probe+'\n')
            result=subprocess.run([sys.executable,__file__],capture_output=True,text=True)
            assert result.returncode!=0,'insensitive gate'
    finally:path.write_text(original)
check()
