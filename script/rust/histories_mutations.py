#!/usr/bin/env python3
"""Remove every remaining eligibility refusal, and each new history behavior, one at a time."""
import pathlib
import re
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parents[2]
logs = root.parent / f'{root.name}-history-mutation-logs'
logs.mkdir(exist_ok=True)

def disk():
    value = subprocess.check_output(['df','-g','/'],text=True)
    print(value,end='',flush=True)
    if int(value.splitlines()[1].split()[3]) < 15:
        raise SystemExit('disk floor: under 15 GB free')

def test(name, path, old, new, expression):
    if len(sys.argv)>1 and name not in sys.argv[1:]: return
    source = root / path
    original = source.read_text()
    assert original.count(old) == 1, (name,original.count(old))
    try:
        source.write_text(original.replace(old,new,1))
        disk()
        with (logs / f'{name}.log').open('w') as output:
            result = subprocess.run(['cargo','nextest','run','-j','5','--test','histories','--test','eligibility','-E',expression],
                                    cwd=root/'rust',stdout=output,stderr=subprocess.STDOUT)
        text=(logs/f'{name}.log').read_text()
        # Compilation errors are not a killed mutant. Require a real failing test.
        assert result.returncode != 0 and 'FAIL' in text and 'test run failed' in text, f'{name} survived or failed to compile: {logs/name}'
        print(f'KILLED {name}',flush=True)
    finally:
        source.write_text(original)

history='test(history_decisions_match)'
new = [
    ('sell','rust/src/engine/basket.rs','Fill::of(order.sell, &order.kind)','Fill::of(false, &order.kind)',history),
    ('normalization','rust/src/figures/fill.rs','let quantity=raw.amount_exec.as_ref().or(if closed{raw.amount.as_ref()}else{None});','if raw.price.is_none() { return Ok(None); }\n    let quantity=raw.amount_exec.as_ref().or(if closed{raw.amount.as_ref()}else{None});',history),
    ('merge_walk','rust/src/engine/basket.rs','for order in orders {','for order in orders { if bot.merged_history() { continue; }',history),
    ('split_tie','rust/src/engine/basket.rs','e.at_us <= created_us','e.at_us < created_us',history),
    ('split_tail','rust/src/engine/basket.rs','for e in pending { apply(&mut ledger, &mut w, e)?; }','for _e in pending {}',history),
    ('carry_boundary','rust/src/figures/fill.rs','created_at>=?2 ORDER BY id','created_at>?2 ORDER BY id',history),
    ('normalized_credit','rust/src/figures/fill.rs','fill.map_or_else(||raw.quote_amount_exec.clone().unwrap_or_else(Dec::zero),|f|f.value)','raw.quote_amount_exec.clone().unwrap_or_else(Dec::zero)',history),
    ('waiting_commitment','rust/src/figures/fill.rs','(None,Some(a),Some(p))=>(a*p)?,','(None,Some(_a),Some(_p))=>fill.as_ref().map_or_else(Dec::zero,|f|f.value.clone()),',history),
    ('reservation','rust/src/figures/fill.rs','let remainder=(&requested-&filled)?;','let remainder=requested; let _observed=filled;',history),
    ('unknown_quantity','rust/src/figures/fill.rs','if fill.is_none() && order.raw.quote_amount_exec.as_ref().is_some_and(Dec::is_positive)', 'if false', 'test(unknown_fill_value)'),
    ('cap_credit','rust/src/figures/fill.rs','rusqlite::types::ValueRef::Null if value.is_zero()=>Num::Int(0)','rusqlite::types::ValueRef::Null=>Num::Int(0)',history),
    ('sell_lift','rust/src/engine/eligibility.rs','if !alpaca && sells > 0','if sells > 0','test(sells_and_merges_pass)'),
    ('merge_lift','rust/src/engine/eligibility.rs','if !alpaca && bot.merged_history()','if bot.merged_history()','test(sells_and_merges_pass)'),
    ('unreadable','rust/src/engine/eligibility.rs','if alpaca && r.is_empty() { super::basket::walk(c, bot, Utc::now())?; }','if alpaca && r.is_empty() {}','test(unknown_fill_value)'),
]
eligibility='rust/src/engine/eligibility.rs'
remaining = [
    ('active_rules', 'if rules > 0', 'if false', 'test(an_active_rule)'),
    ('unreadable_report', 'report.unreadable.push((id, format!("{e:?}")));', 'let _observed = (id,e);', 'test(unknown_fill_value)'),
    ('working_report', 'report.problems.push(format!("bot {id} ({}): {}", bot.status.label(), reasons.join(", ")));', 'let _observed = &reasons;', 'test(deferred_classes_and_existing_refusals)'),
    ('idle_work_report', 'if !work.is_empty() {', 'if false {', 'test(a_stopped_bot_outside_the_slice)'),
    ('stale_report', 'report.problems.push(format!("bot {id} ({}): {}", bot.status.label(), s.message));', 'let _observed = s;', 'test(check_and_the_takeover_refuse)'),
    ('problems_refusal', 'if !self.problems.is_empty()', 'if false', 'test(a_report_refuses_problems_first)'),
    ('unreadable_refusal', 'if !self.unreadable.is_empty()', 'if false', 'test(a_report_refuses_problems_first)'),
    ('guard_rules', 'check_install(tx).map_err(failed)?.refusal()?;', 'check_install(tx).map_err(failed)?;', 'test(unknown_fill_value)'),
    ('guard_stranded', 'if !stranded.is_empty()', 'if false', 'test(a_legacy_intent_gets_its_snapshot)'),
    ('guard_preflight', 'crate::venue::alpaca::preflight(tx, cipher).map(|_| ()).map_err(Refusal::Untradable)', 'Ok(())', 'test(the_guard_refuses_a_start_this_build_cannot_trade)'),
]
for key in ['rebalance_pending','liquidation_pending','redeploy_pending']:
    remaining.append((key, f'"{key}"', f'"mutant_{key}"', 'test(deferred_classes_and_existing_refusals)'))
for name,old,replacement,expression in remaining:
    new.append((name,eligibility,old,replacement,expression))
for args in new: test(*args)

# Balanced-parenthesis scanning finds each refusal invocation, including multiline format!s.
relative='rust/src/engine/eligibility.rs'
text=(root/relative).read_text()
starts=list(re.finditer(r'(?<![\w.])r\.push\(',text))
for number,match in enumerate(starts):
    start=match.start(); cursor=match.end(); depth=1; quoted=False; escaped=False
    while depth:
        ch=text[cursor]; cursor+=1
        if quoted:
            if escaped: escaped=False
            elif ch=='\\': escaped=True
            elif ch=='"': quoted=False
        elif ch=='"': quoted=True
        elif ch=='(': depth+=1
        elif ch==')': depth-=1
    call=text[start:cursor]
    # Repeated r.push(e) has different branches: use its complete source line as the anchor.
    if text.count(call)>1:
        begin=text.rfind('\n',0,start)+1; end=text.find('\n',cursor)
        old=text[begin:end]; replacement=old.replace(call,call.replace('r.push','(|_: String| ())'),1)
    else:
        old=call; replacement=call.replace('r.push','(|_: String| ())',1)
    test(f'refusal_{number:02}',relative,old,replacement,
         'test(every_remaining_eligibility_branch) | test(deferred_classes_and_existing_refusals)')
print(f'PASS selected mutations' if len(sys.argv)>1 else f'PASS {len(new)} behavior/global mutations and {len(starts)} per-branch refusal mutations',flush=True)
