#!/usr/bin/env python3
"""Check final literal blocks and every untouched tracked file against the pinned archive."""
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile

root = Path(__file__).resolve().parents[2]
manifest = json.loads((root / 'script/rust/settings_files.json').read_text())
for name, expected in manifest['files'].items():
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == expected, name
source = Path(os.environ['S1_SOURCE'])
assert subprocess.check_output(['git', '-C', str(source), 'status', '--porcelain']) == b'', 'source worktree changed'
archive = subprocess.check_output(['git', '-C', str(source), 'archive', manifest['base']])
count = 0
with tarfile.open(fileobj=io.BytesIO(archive)) as original:
    for member in original.getmembers():
        if not member.isfile():
            continue
        count += 1
        if member.name in manifest['files']:
            continue
        stream = original.extractfile(member)
        assert stream is not None
        assert (root / member.name).read_bytes() == stream.read(), member.name
assert count == manifest['tracked_count'], (count, manifest['tracked_count'])
print(f'PASS: {len(manifest["files"])} literal files and all {count} tracked files; source worktree clean')
