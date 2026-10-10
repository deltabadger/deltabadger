#!/usr/bin/env python3
"""R2 grep gate: audit the complete venue-text sink graph and merged upstream routes."""
import json
from pathlib import Path
import sys
root=Path(__file__).resolve().parents[2]
contract=json.loads((root/'script/rust/settings_r2_sinks.json').read_text())
def errors():
    return [row['label'] for row in contract if (root/'rust/src'/row['file']).read_text().count(row['anchor'])!=row['count']]
if '--self-test' in sys.argv:
    for row in contract:
        path=root/'rust/src'/row['file'];original=path.read_bytes()
        try:
            path.write_text(original.decode().replace(row['anchor'], '/* R2 sink bypass probe */',1))
            assert row['label'] in errors(), row['label']
        finally:path.write_bytes(original)
    print(f"PASS: all {len(contract)} independent R2 sink/route grep probes rejected; exact source restored")
assert not errors(), errors()
print(f"PASS: all {len(contract)} R2 venue-text sink and current-base route contracts")
