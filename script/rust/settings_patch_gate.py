#!/usr/bin/env python3
"""Apply the plan's exact shared diffs with unrelated additions; allow refusal, never deletion."""
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
root=Path(__file__).resolve().parents[2]
manifest=json.loads((root/'script/rust/settings_files.json').read_text())
patches=json.loads((root/'script/rust/settings_patches.json').read_text())
blob=subprocess.check_output(['git','-C',os.environ['S1_SOURCE'],'archive',manifest['base']])
with tarfile.open(fileobj=io.BytesIO(blob)) as tar:
    original={member.name:tar.extractfile(member).read() for member in tar.getmembers() if member.isfile()}
marker=b'\n// R2 unrelated upstream addition must survive\n'
shared=set(row['file'] for row in patches if row['file'] in original)
refusals=set()
with tempfile.TemporaryDirectory(prefix='s1-patch-gate-',dir=os.environ['S1_EVIDENCE']) as directory:
    work=Path(directory)
    for name in shared:
        path=work/name;path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(original[name]+marker)
    for row in patches:
        name=row['file']
        if name not in shared or name in refusals:continue
        checked=subprocess.run(['git','apply','--check','-'],input=row['patch'].encode(),cwd=work,capture_output=True)
        if checked.returncode:
            refusals.add(name)
        else:subprocess.run(['git','apply','-'],input=row['patch'].encode(),cwd=work,check=True,capture_output=True)
        assert marker in (work/name).read_bytes(),name
    assert len(shared)>40 and 'rust/src/web/mod.rs' in shared
    if '--self-test' in sys.argv:
        for name in sorted(shared):
            path=work/name;green=path.read_bytes()
            try:
                # The prohibited full-file replacement would copy the final file and delete unrelated upstream code.
                path.write_bytes((root/name).read_bytes())
                try:assert marker in path.read_bytes(),name
                except AssertionError:pass
                else:raise AssertionError('full-file replacement survived: '+name)
            finally:path.write_bytes(green)
        print(f'PASS: all {len(shared)} full-file replacement mutations refused by preservation test')
print(f'PASS: {len(patches)} literal shared patches; {len(shared)} current-base files preserve unrelated additions ({len(refusals)} safely refuse changed context)')
