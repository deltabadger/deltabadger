#!/usr/bin/env python3
"""Real executable key-branch and guard mutations; restore files even on failure."""
import os
import pathlib
import signal
import subprocess
import time

root = pathlib.Path(__file__).resolve().parents[2]
control = root / 'rust/src/web/mcp/control.rs'
writer = root / 'rust/src/web/bot/write.rs'
parser = root / 'rust/src/web/bot/mcp_input.rs'
original = {p: p.read_text() for p in [control, writer, parser]}
cases = [('stop_bot', 'stop_bot_status_1', 'Stop', 'Archive'),
         ('archive_bot', 'archive_bot_status_2', 'Archive', 'Stop'),
         ('unarchive_bot', 'unarchive_bot_status_7', 'Unarchive', 'Archive'),
         ('delete_bot', 'delete_bot_status_2', 'Delete', 'Stop'),
         ('update_bot_settings', 'settings_label', None, None),
         ('start_bot', 'start_bot_status_0', 'Start', 'Stop')]


def run(label, binary, test, env, marker):
    free = os.statvfs('/data')
    assert free.f_bavail * free.f_frsize >= 20 * 1024**3, 'STOP: under 20 GiB free'
    subprocess.run(['df', '-h', '/data'], check=True)
    cmd = ['cargo', 'nextest', 'run', '--manifest-path', 'rust/Cargo.toml', '--locked',
           '-j', '6', '--test', binary, '-E', f'test({test})']
    process = subprocess.Popen(cmd, cwd=root, env=env, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, text=True, start_new_session=True)
    try:
        output, _ = process.communicate(timeout=180)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.communicate()
        raise AssertionError(f'{label}: timed out, not a test failure')
    evidence = pathlib.Path(os.environ['M4_EVIDENCE']) / 'mutations'
    evidence.mkdir(exist_ok=True)
    (evidence / (label.replace(' ', '_') + '.log')).write_text(output)
    assert process.returncode != 0 and 'Compiling' in output and 'FAIL [' in output and marker in output, f'{label}: wrong outcome\n{output}'
    assert 'could not compile' not in output, f'{label}: compile failure\n{output}'
    print(f'PASS sensitivity {label}: intended assertion failed', flush=True)


try:
    for tool, scenario, action, alternative in cases:
        env = dict(os.environ, M4=scenario)
        if action:
            needle = f'"{tool}"=>Action::{action}'
            replacement = f'"{tool}"=>Action::{alternative}'
        else:
            needle = 'if name=="update_bot_settings"'
            replacement = 'if false'
        assert original[control].count(needle) == 1
        control.write_text(original[control].replace(needle, replacement))
        run(f'{tool} key branch', 'mcp_control', 'control_transcripts', env, 'M4 mismatches')
        control.write_text(original[control])

        # Only this tool's writer guard is removed. Other tools retain the guard;
        # the six-tool rollback test must pinpoint this one's incorrect success.
        if tool == 'update_bot_settings':
            needle = 'if let Err(refusal) = eligibility::guard(&tx, &ctx.app.cipher, Some(id))'
            replacement = 'if let Err(refusal) = Ok::<(),eligibility::Refusal>(())'
        else:
            needle = 'if let Err(refusal)=eligibility::guard(&tx,&ctx.app.cipher,Some(id))'
            replacement = f'if let Err(refusal)=if submitted.mcp() && action==Action::{action} {{Ok(())}} else {{eligibility::guard(&tx,&ctx.app.cipher,Some(id))}}'
        assert original[writer].count(needle) == 1
        writer.write_text(original[writer].replace(needle, replacement))
        run(f'{tool} guard', 'mcp_control_races', 'each_write_rolls_back_on_guard_refusal', dict(os.environ), tool)
        writer.write_text(original[writer])
    # Flip every exhaustive Outcome arm independently, including unchanged/success.
    for variant in ['Missing', 'Unported', 'GuardRefused', 'Invalid', 'NoChange', 'Committed']:
        arm = next(line for line in original[control].splitlines() if line.strip().startswith('Outcome::'+variant+'(') or line.strip().startswith('Outcome::'+variant+'=>'))
        value = 'true' if variant in ['Unported', 'GuardRefused'] else 'false'
        flipped = arm.replace(','+value+')', ','+('false' if value=='true' else 'true')+')')
        assert arm != flipped
        control.write_text(original[control].replace(arm, flipped))
        if variant == 'Committed':
            run('R1 '+variant+' isError', 'mcp_control', 'control_transcripts', dict(os.environ,M4='settings_label'), 'M4 mismatches')
        else:
            run('R1 '+variant+' isError', 'mcp_control_races', 'r1_refusal_branches_report_error_and_rollback', dict(os.environ), 'R1 refusal')
        control.write_text(original[control])

    writer.write_text(original[writer].replace('writer_refused = true;', 'writer_refused = false;'))
    run('R1 key refusal routing', 'mcp_control_races', 'r1_refusal_branches_report_error_and_rollback', dict(os.environ), 'R1 refusal missing_key')
    writer.write_text(original[writer])

    needle = "text.trim_matches(|c:char| matches!(c,'\\0'|'\\t'|'\\n'|'\\u{000b}'|'\\u{000c}'|'\\r'|' '))"
    assert original[parser].count(needle) == 1
    for label, replacement, scenario in [
        ('Unicode strip', 'text.trim()', 'settings_r1_nbsp_'),
        ('NUL strip', needle.replace("'\\0'|", ''), 'settings_r1_nul_'),
        ('TAB strip', needle.replace("'\\t'|", ''), 'settings_r1_tab_')]:
        parser.write_text(original[parser].replace(needle,replacement))
        run('R1 '+label, 'mcp_control', 'control_transcripts', dict(os.environ,M4=scenario), 'M4 mismatches')
        parser.write_text(original[parser])

    needle = 'if view.errors.is_empty() && (!safety || submitted.mcp() && action != Action::Stop) {'
    assert original[writer].count(needle) == 1
    for action in ['Archive','Unarchive','Delete','Stop']:
        replacement = ('if view.errors.is_empty() && (!safety || submitted.mcp()) {' if action=='Stop' else
                       needle.replace(' {', f' && !(submitted.mcp() && action == Action::{action}) {{'))
        writer.write_text(original[writer].replace(needle,replacement))
        run('R1 invalid-row '+action, 'mcp_control_races', 'r1_invalid_lifecycle_rows_validate_but_stop', dict(os.environ), 'R1 lifecycle')
        writer.write_text(original[writer])

    needle = 'draft.candidate.transient.insert("missed_quote_amount".into(), serialized(carry.clone())?);'
    assert original[writer].count(needle) == 1
    writer.write_text(original[writer].replace(needle, 'draft.candidate.transient.insert("missed_quote_amount".into(), draft.raw_transient.get("missed_quote_amount").cloned().unwrap_or(serde_json::Value::Null));'))
    run('R2 settings carry order', 'mcp_control', 'control_transcripts', dict(os.environ,M4='settings_r2_negative_carry_'), 'M4 mismatches')
    writer.write_text(original[writer])

    needle = 'draft.candidate.started_at=Some(ctx.now);\n            if draft.candidate.transient.contains_key("last_action_job_at") {\n                draft.candidate.transient.insert("last_action_job_at".into(),Value::Null);\n            }\n            draft.candidate.transient.insert("missed_quote_amount".into(),Value::Null);'
    assert original[writer].count(needle) == 1
    writer.write_text(original[writer].replace(needle, needle.replace('"missed_quote_amount".into(),Value::Null', '"missed_quote_amount".into(),draft.raw_transient.get("missed_quote_amount").cloned().unwrap_or(Value::Null)')))
    run('R2 fresh start carry order', 'mcp_control', 'control_transcripts', dict(os.environ,M4='start_bot_r2_negative_carry'), 'M4 mismatches')
    writer.write_text(original[writer])

    needle = 's.eq_ignore_ascii_case(&name)'
    assert original[parser].count(needle) == 1
    parser.write_text(original[parser].replace(needle, 's.to_uppercase()==name.to_uppercase()'))
    run('R2 Unicode symbol acceptance', 'mcp_control', 'control_transcripts', dict(os.environ,M4='settings_r2_unicode_symbol'), 'R2 Unicode ASCII-only refusal')
    parser.write_text(original[parser])

    needle = 'super::mcp_input::InputOutcome::AsciiAliasRefused => mcp_refused = true'
    assert original[writer].count(needle) == 1
    writer.write_text(original[writer].replace(needle, needle.replace('= true','= false')))
    run('R2 Unicode refusal classification', 'mcp_control', 'control_transcripts', dict(os.environ,M4='settings_r2_unicode_symbol'), 'R2 Unicode refusal classification')
    writer.write_text(original[writer])

finally:
    for path, source in original.items():
        path.write_text(source)
print('PASS all 30 executable mutations; sources restored', flush=True)
