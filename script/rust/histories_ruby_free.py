#!/usr/bin/env python3
"""Run the actual workflow's test command with Ruby absent from PATH."""
import os
import pathlib
import shlex
import shutil
import subprocess
import tempfile

root=pathlib.Path(__file__).resolve().parents[2]
workflow=(root/'.github/workflows/rust.yml').read_text()
block=workflow.split('      - name: Test\n',1)[1].split('        run: >\n',1)[1]
lines=[]
for line in block.splitlines():
    if line and not line.startswith('          '): break
    lines.append(line.strip())
command=shlex.split(' '.join(lines))
assert command[:2]==['cargo','test']
assert '--test' in command and 'histories_sources' in command
with tempfile.TemporaryDirectory(prefix='no-ruby-',dir=root.parent) as tmp:
    for name in ['cargo','rustc','rustdoc','cc','clang','ar','xcrun','bash','sh','env','git','python3','uname','ld']:
        source=shutil.which(name)
        if source: pathlib.Path(tmp,name).symlink_to(source)
    env=os.environ.copy()
    env['PATH']=tmp
    assert shutil.which('ruby',path=tmp) is None
    assert shutil.which('bundle',path=tmp) is None
    print('Ruby and Bundler absent from PATH; executing the workflow command',flush=True)
    subprocess.run(command,cwd=root/'rust',env=env,check=True)
