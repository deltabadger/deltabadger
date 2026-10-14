#!/usr/bin/env python3
"""script/rust/repin on a throwaway clone of this checkout: python3 script/rust/repin_test.py
The re-record cases run Rails (bin/rails runner), so they need the app's Ruby and gems."""
import os, shutil, subprocess, tempfile, unittest

ROOT = subprocess.run(['git', 'rev-parse', '--show-toplevel'], capture_output=True, text=True, check=True,
                      cwd=os.path.dirname(os.path.abspath(__file__))).stdout.strip()
HASH_ONLY_FILE = 'app/views/bots/_assets_table.html.erb'     # pinned only by pin lists
RECORDED_FILE = 'app/models/automation/statusable.rb'        # pinned only by run_ownership.json
RECORDER = 'script/rust/record_run_ownership.rb'
FIXTURE = 'rust/tests/fixtures/run_ownership.json'

class Repin(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.dir = tempfile.mkdtemp(prefix='repin-test.')
        subprocess.run(['git', 'clone', '-q', '--shared', ROOT, cls.dir], check=True)
        # The script under test is this checkout's, committed or not.
        shutil.copyfile(os.path.join(ROOT, 'script/rust/repin'), os.path.join(cls.dir, 'script/rust/repin'))
        cls.git('-c', 'user.name=t', '-c', 'user.email=t@t', 'commit', '-qam', 'repin under test', '--allow-empty')

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(cls.dir, ignore_errors=True)

    def setUp(self):
        self.git('reset', '-q', '--hard')
        self.git('clean', '-qfd')

    @classmethod
    def git(cls, *args):
        return subprocess.run(['git', *args], cwd=cls.dir, capture_output=True, text=True, check=True).stdout

    def repin(self, *args):
        done = subprocess.run(['script/rust/repin', *args], cwd=self.dir, capture_output=True, text=True)
        return done.returncode, done.stdout + done.stderr

    def append(self, path, text):
        with open(os.path.join(self.dir, path), 'a') as f: f.write(text)

    def changed(self):
        return sorted(self.git('diff', '--name-only').split())

    def test_a_current_tree_is_left_alone(self):
        self.assertEqual(self.repin('--check'), (0, ''))
        code, out = self.repin()
        self.assertEqual((code, out), (0, 'every pin is current\n'))
        self.assertEqual(self.git('status', '--porcelain'), '')

    def test_a_pin_list_is_re_hashed(self):
        self.append(HASH_ONLY_FILE, '\n')
        code, out = self.repin('--check')
        self.assertEqual(code, 1)
        self.assertIn(f'{HASH_ONLY_FILE}  (re-hash)', out)
        code, out = self.repin()
        self.assertEqual(code, 0, out)
        self.assertIn(f're-hashed    rust/tests/fixtures/page_figure_sources.json  {HASH_ONLY_FILE}', out)
        self.assertNotIn('re-record', out)
        self.assertEqual(self.repin('--check'), (0, ''))
        self.assertEqual(self.changed(), [HASH_ONLY_FILE, 'rust/tests/fixtures/page_figure_sources.json'])
        self.assertEqual(self.git('diff', '--numstat', '--', 'rust/tests/fixtures/page_figure_sources.json').split()[:2], ['1', '1'])

    def test_a_recorded_fixture_is_re_recorded_from_rails(self):
        self.append(RECORDED_FILE, '# touched\n')
        code, out = self.repin('--check')
        self.assertEqual(code, 1)
        self.assertIn(f'stale  {FIXTURE}  {RECORDED_FILE}  (re-record)', out)
        code, out = self.repin()
        self.assertEqual(code, 0, out)
        self.assertIn(f're-recorded  {FIXTURE}  {RECORDED_FILE}', out)
        self.assertIn('content unchanged', out)
        self.assertEqual(self.repin('--check'), (0, ''))
        self.assertEqual(self.changed(), [RECORDED_FILE, FIXTURE])

    def test_a_re_recording_that_moves_the_content_says_so(self):
        # Rails "behaves differently": the recorder now records a case under another name.
        path = os.path.join(self.dir, RECORDER)
        with open(path) as f: text = f.read()
        with open(path, 'w') as f: f.write(text.replace("'rails_quote_amount' =>", "'rails_quote_amount_moved' =>"))
        code, out = self.repin()
        self.assertEqual(code, 0, out)
        self.assertIn('CONTENT CHANGED: 2 values moved', out)
        self.assertIn('cases/settings_held_until_reconciliation/rails_quote_amount_moved', out)
        self.assertEqual(self.repin('--check'), (0, ''))

    def test_a_failing_recorder_leaves_the_fixture_stale(self):
        self.append(RECORDER, '\nraise "recorder broke"\n')
        code, out = self.repin()
        self.assertEqual(code, 1)
        self.assertIn(f'FAILED       {FIXTURE}', out)
        self.assertIn('recorder broke', out)
        self.assertEqual(self.changed(), [RECORDER])
        self.assertEqual(self.repin('--check')[0], 1)

if __name__ == '__main__':
    unittest.main()
