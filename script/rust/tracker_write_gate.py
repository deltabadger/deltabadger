"""D5 additions obey the external-data/error and fill-normalizer boundaries."""
from pathlib import Path
import re
import subprocess
import sys

root = Path(__file__).resolve().parents[2]
source = root / 'rust/src/web/tracker.rs'
pattern = re.compile(r'\.unwrap\s*\(|\.expect\s*\(|\.unwrap_or_default\s*\(|\blet\s+_\s*=|\bpanic!')


def check():
    subprocess.run([sys.executable, str(root / 'script/rust/mcp_reads_gate.py')], check=True)
    for path in [source, *sorted(source.with_suffix('').rglob('*.rs'))]:
        code = path.read_text().split('#[cfg(test)]')[0]
        for line in code.splitlines():
            if not line.strip().startswith('//'):
                assert not pattern.search(line), 'new unchecked/discarded write path: ' + line
        assert not re.search(r'\b(?:amount_exec|quote_amount_exec)\b|\.raw\.', code), 'raw fill read outside shared normalizer'
        assert not re.search(r'\.ok\s*\(', code.replace('h.to_str().ok()', 'HEADER_UTF8')), 'discarded error in tracker writer'
    for path in sorted((root/'rust/src/web').rglob('*.rs')):
        for line in path.read_text().splitlines():
            # The modal year is the sole non-ID Ruby integer conversion in web/.
            if 'ruby::to_i' in line:
                assert path.name=='modal.rs' and 'let selected=' in line and 'settings["year"]' in line, 'unchecked ID conversion in web/: '+str(path)
    print('PASS D5 tracker writer error, checked-ID and raw-fill gates')


if '--sensitivity' in sys.argv:
    original = source.read_text()
    try:
        for probe, marker in [
            ('fn probe() { let _ = db.execute("UPDATE users", []); }', 'unchecked/discarded'),
            ('fn probe() { let sql = "SELECT amount_exec FROM transactions"; }', 'raw fill read'),
            ('fn probe() { let result = db.execute("UPDATE users", []).ok(); }', 'discarded error'),
            ('fn probe(id: &str) { let id = crate::ruby::to_i(id); }', 'unchecked ID conversion'),
        ]:
            source.write_text(original + '\n' + probe + '\n')
            answer = subprocess.run([sys.executable, __file__], capture_output=True, text=True)
            assert answer.returncode != 0 and marker in answer.stderr, answer.stderr
    finally:
        source.write_text(original)
    print('PASS D5 gate sensitivity; source restored')
check()
