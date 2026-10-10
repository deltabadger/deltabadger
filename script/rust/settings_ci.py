#!/usr/bin/env python3
"""Execute exactly the workflow's Rails-free targets with Ruby absent from PATH."""
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess

root = Path(__file__).resolve().parents[2]
workflow = (root / '.github/workflows/rust.yml').read_text()
section = workflow.split('      - name: Test\n', 1)[1]
match = re.match(r'        run: >\n((?:          [^\n]+\n?)+)', section)
assert match, 'workflow Test command changed'
command = shlex.split(' '.join(line.strip() for line in match[1].splitlines()))
assert command[:2] == ['cargo', 'test'], command
assert '--test' in command and command[-4:] == ['--test', 'settings', '--test', 's1_http_clock'], command
# Keep target selection identical, using nextest's six-process limit for this machine.
command = ['cargo', 'nextest', 'run', '--manifest-path', 'rust/Cargo.toml',
           '--config-file', '.config/nextest.toml', '-j', '6', '--no-fail-fast'] + command[2:]
assert 'S1_R4_MUTATION' not in os.environ, 'restored CI must run the Cargo provenance gate'
environment = os.environ.copy()
environment['PATH'] = str(Path.home() / '.cargo/bin') + ':/usr/bin:/bin'
assert shutil.which('ruby', path=environment['PATH']) is None, 'Ruby must be off PATH'
assert shutil.which('bundle', path=environment['PATH']) is None, 'Bundler must be off PATH'
free = os.statvfs('/data')
assert free.f_bavail * free.f_frsize >= 20 * 1024**3, 'STOP: under 20 GiB free'
subprocess.run(['df', '-h', '/data'], check=True)
print('Ruby and Bundler absent from PATH; exact CI target selection:', ' '.join(command), flush=True)
subprocess.run(command, cwd=root, env=environment, check=True)
