#!/usr/bin/env python3
"""Executable M3 read, permission, accounting and bound mutations, with restoration."""
import os
import pathlib
import signal
import subprocess

root = pathlib.Path(__file__).resolve().parents[2]
cases = []
def add(label, file, before, after, binary, test, marker):
    cases.append((label, root / file, before, after, binary, test, marker))
reads = 'rust/src/web/mcp/reads.rs'
tools = 'rust/src/web/mcp/tools.rs'
limits = 'rust/src/web/mcp/read_limits.rs'
fill = 'rust/src/figures/fill.rs'
contract = 'mcp_reads_contract'
for tool in ['get_exchange_balances', 'list_open_orders', 'get_bot_details', 'get_portfolio_summary']:
    add(tool + ' branch', reads, '"' + tool + '"=>', '"removed_' + tool + '"=>', 'mcp_reads', 'four_reads_match_rails', 'order row cap' if tool=='list_open_orders' else 'normalized $230/+15% required' if tool=='get_portfolio_summary' else 'm3_owner')
    add(tool + ' permission', tools, '    let (enabled,granted) = consent::mcp_access(c,who.user_id,who.application_id)?;\n    Ok(if', '    if name=="' + tool + '" {return Ok(None)}\n    let (enabled,granted) = consent::mcp_access(c,who.user_id,who.application_id)?;\n    Ok(if', contract, 'every_read_is_hidden_until_granted_and_removed_when_disabled', 'called `Option::unwrap()` on a `None` value')
add('row cap', limits, 'n<=limit', 'n<limit', contract, 'every_row_cap_accepts_the_boundary_and_refuses_the_next_row', 'at')
add('stored string cap', limits, 'size>STRING', 'size>=STRING', contract, 'stored_strings_are_bounded_in_bytes_before_they_are_loaded', 'assertion')
add('aggregate bytes cap', limits, '*total>STORED_BYTES', '*total>=STORED_BYTES', contract, 'aggregate_stored_bytes_are_bounded', 'assertion')
add('text cap', limits, 's.len()>TEXT', 's.len()>=TEXT', contract, 'final_text_is_bounded_in_bytes_without_truncation', 'assertion')
add('bot budget', limits, 'budget::charge(1,0)', 'budget::charge(0,0)', contract, 'preliminary_bot_work_consumes_the_figures_budget', 'assertion')
add('index metadata admission', limits, '("indices","?1 IS NOT NULL",CATALOG)', '("indices","?1 IS NULL",CATALOG)', contract, 'generated_index_labels_cannot_load_unbounded_index_metadata', 'index metadata')
add('venue nil price', 'rust/src/venue/alpaca.rs', 'order.price = filled.filter(BigDec::is_positive).or(limit);', 'order.price = filled.filter(BigDec::is_positive).or(limit).or_else(||Some(BigDec::zero()));', contract, 'read_order_price_absence_does_not_change_engine_polling', 'assertion')
add('stored transaction zero price', tools, 'number(r,3)?,r.get::<_,Option<String>>(4)?,price(r,5)?', 'number(r,3)?,r.get::<_,Option<String>>(4)?,number(r,5)?', 'mcp_reads', 'mcp_transaction_zero_price_uses_the_nil_branch', 'stored 0 is unknown')
add('polled open-order zero price', reads, 'tools::price(r,6)?', 'tools::number(r,6)?', 'mcp_reads', 'polled_open_market_zero_price_uses_the_nil_branch', 'assertion')
add('effective fill quantity', fill, 'raw.amount_exec.as_ref().or(if order.closed{raw.amount.as_ref()}else{None})', 'raw.amount.as_ref().or(if order.closed{raw.amount_exec.as_ref()}else{None})', contract, 'fill_completeness_covers_every_quantity_value_column_combination', 'assertion')
add('closed fill fallback', fill, 'if order.closed{raw.amount.as_ref()}else{None}', 'if !order.closed{raw.amount.as_ref()}else{None}', contract, 'fill_completeness_covers_every_quantity_value_column_combination', 'assertion')
add('fill positive quantity', fill, 'quantity.filter(|q|q.is_positive())', 'quantity.filter(|q|!q.is_zero())', contract, 'fill_completeness_covers_every_quantity_value_column_combination', 'assertion')
add('fill reported value priority', fill, 'raw.quote_amount_exec.as_ref().filter(|v|v.is_positive())', 'raw.price.as_ref().filter(|v|v.is_positive())', contract, 'fill_completeness_covers_every_quantity_value_column_combination', 'assertion')
add('fill missing value refusal', fill, 'return Err(FiguresError::NotComputed("executed fill value unavailable".into()))', 'return Ok(None)', contract, 'fill_completeness_covers_every_quantity_value_column_combination', 'assertion')
add('fill exact value multiplication', fill, '(price*quantity)?', '(price+quantity)?', contract, 'normalized_fill_multiplies_decimals_without_float_rounding', 'assertion')
add('bot owner lookup', reads, 'b.id=?1 AND b.user_id=?2 AND b.status!=3', 'b.id=?1 AND ?2 IS NOT NULL AND b.status!=3', 'mcp_reads', 'four_reads_match_rails', 'm3_unknown_bot')
add('open order deduplication', reads, 'if ids.contains(id){continue}', 'if false && ids.contains(id){continue}', 'mcp_reads', 'four_reads_match_rails', 'm3_deduplicate')
original = {path: path.read_text() for _, path, *_ in cases}
evidence = pathlib.Path(os.environ['M4_EVIDENCE']) / 'm3-mutations'
evidence.mkdir(exist_ok=True)
try:
    for label, path, before, after, binary, test, marker in cases:
        assert original[path].count(before) == 1, (label, before)
        assert os.statvfs('/data').f_bavail * os.statvfs('/data').f_frsize >= 20 * 1024**3
        subprocess.run(['df', '-h', '/data'], check=True)
        path.write_text(original[path].replace(before, after))
        cmd = ['cargo', 'nextest', 'run', '--manifest-path', 'rust/Cargo.toml', '--locked', '-j', '6', '--test', binary, '-E', f'test(={test})']
        process = subprocess.Popen(cmd, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, start_new_session=True)
        try:
            output, _ = process.communicate(timeout=600)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.communicate(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.communicate()
            raise AssertionError(label + ': timeout is not an assertion failure')
        (evidence / (label.replace(' ', '_') + '.log')).write_text(output)
        assert process.returncode != 0 and 'FAIL [' in output and marker in output and 'could not compile' not in output, label + '\n' + output
        path.write_text(original[path])
        print('PASS M3 sensitivity ' + label + ': intended assertion failed', flush=True)
finally:
    for path, source in original.items():
        path.write_text(source)
print(f'PASS all {len(cases)} M3 executable mutations; sources restored', flush=True)
