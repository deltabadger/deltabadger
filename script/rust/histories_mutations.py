#!/usr/bin/env python3
"""Remove every remaining eligibility refusal, and each new history behavior, one at a time."""
import pathlib
import re
import subprocess
import sys
import json,hashlib
import os,signal,time

root = pathlib.Path(__file__).resolve().parents[2]
logs = root.parent / f'{root.name}-history-mutation-logs'
logs.mkdir(exist_ok=True)

def disk(name):
    value = subprocess.check_output(['df','-g','/'],text=True)
    print(value,end='',flush=True)
    if int(value.splitlines()[1].split()[3]) < 15:
        raise SystemExit(f'NOT KILLED {name}: disk floor under 15 GB free')

# R8b: each mutant has a separate process group and a hard wall-clock budget.
# A stopped/timeout run is never evidence of a killed mutant.
MUTANT_TIMEOUT = 300
active = logs / 'active.json'

def resources():
    available = int(subprocess.check_output(['df','-g','/'],text=True).splitlines()[1].split()[3])
    swap = subprocess.check_output(['sysctl','vm.swapusage'],text=True).strip()
    used = float(re.search(r'used = ([0-9.]+)M',swap).group(1))
    return {'free_gb':available,'swap_used_mb':used,'swap':swap}

def run_mutant(name, command, output, started):
    process = None
    reason = None
    minimum = None
    peak_swap = 0.0
    previous = {}
    def interrupted(signum, frame):
        raise SystemExit(f'NOT KILLED {name}: interrupted by signal {signum}')
    for sig in (signal.SIGINT,signal.SIGTERM):
        previous[sig] = signal.signal(sig,interrupted)
    try:
        with (logs/f'{name}-resources.jsonl').open('a') as samples:
            while True:
                sample = resources()
                minimum = sample['free_gb'] if minimum is None else min(minimum,sample['free_gb'])
                peak_swap = max(peak_swap,sample['swap_used_mb'])
                elapsed = time.time()-started
                samples.write(json.dumps({'elapsed_s':elapsed,**sample})+'\n');samples.flush()
                if sample['free_gb'] < 15:
                    reason = 'disk floor below 15 GB'
                elif elapsed >= MUTANT_TIMEOUT:
                    reason = '300-second wall-clock timeout'
                if reason:
                    raise SystemExit(f'NOT KILLED {name}: {reason}')
                if process is None:
                    process = subprocess.Popen(command,cwd=root/'rust',stdout=output,stderr=subprocess.STDOUT,start_new_session=True)
                    temp = active.with_suffix('.tmp')
                    temp.write_text(json.dumps({'name':name,'pgid':process.pid,'timeout_s':MUTANT_TIMEOUT})+'\n')
                    temp.replace(active)
                try:
                    code = process.wait(timeout=min(0.5,MUTANT_TIMEOUT-elapsed))
                except subprocess.TimeoutExpired:
                    continue
                if time.time()-started >= MUTANT_TIMEOUT:
                    reason = '300-second wall-clock timeout'
                    raise SystemExit(f'NOT KILLED {name}: {reason}')
                if code < 0 or code in (124,137,143):
                    reason = f'process killed (exit {code})'
                    raise SystemExit(f'NOT KILLED {name}: {reason}')
                return subprocess.CompletedProcess(command,code)
    except BaseException as error:
        if reason is None: reason = str(error)
        raise
    finally:
        if process is not None:
            # Kill immediately, including descendants; never wait for a test step first.
            try: os.killpg(process.pid,signal.SIGKILL)
            except ProcessLookupError: pass
            process.wait()
        active.unlink(missing_ok=True)
        for sig,handler in previous.items(): signal.signal(sig,handler)
        with (logs/f'{name}-runs.jsonl').open('a') as report:
            report.write(json.dumps({'name':name,'command':command,'pgid':None if process is None else process.pid,
                'exit_code':None if process is None else process.returncode,'stop_reason':reason,
                'elapsed_s':time.time()-started,'minimum_free_gb':minimum,'peak_swap_used_mb':peak_swap})+'\n')

def test(name, path, old, new, expression):
    if len(sys.argv)>1 and name not in sys.argv[1:]: return
    started = time.time()
    source = root / path
    original = source.read_text()
    assert original.count(old) == 1, (name,original.count(old))
    primitive=root/"script/rust/histories_arithmetic_primitives.json"
    original_primitive=primitive.read_text()
    timestamp_gate=root/'script/rust/histories_timestamp_gate.py'
    original_timestamp_gate=timestamp_gate.read_text()
    try:
        if name=='r9_writer_strict_timestamp':
            # Prove the reviewer regression itself catches the old bug, independently
            # of the structural gate (whose sensitivity separately rejects this parser).
            timestamp_gate.write_text('print("R9 runtime mutation: structural gate tested separately")\n')
        source.write_text(original.replace(old,new,1))
        if path in ("rust/src/ruby.rs","rust/src/codec.rs"):
            hashes=json.loads(original_primitive);hashes[path]=hashlib.sha256(source.read_bytes()).hexdigest()
            primitive.write_text(json.dumps(hashes,indent=2)+"\n")
        disk(name)
        with (logs / f'{name}.log').open('w') as output:
            result = run_mutant(name, ['cargo','nextest','run','-j','5','--test','histories','--test','eligibility','--test','histories_r1','--test','amount','--test','histories_sources','--test','web_actions','--lib','-E',expression],
                                    output, started)
        text=(logs/f'{name}.log').read_text()
        # Compilation errors are not a killed mutant. Require a real failing test.
        assert result.returncode == 100 and 'FAIL' in text and 'test run failed' in text, f'{name} survived or failed to compile: {logs/name}'
        print(f'KILLED {name}',flush=True)
    finally:
        source.write_text(original)
        primitive.write_text(original_primitive)
        timestamp_gate.write_text(original_timestamp_gate)

new = [
    ('r9c_calendar_range', 'rust/src/codec.rs', '    let sql = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")', '    if s.starts_with([\'-\',\'+\']) { return Err(CodecError::Time("unreadable stored timestamp".into())); }\n    let sql = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")', 'test(r9c_sql_timestamp)'),
    ('r9c_eager_loader', 'rust/src/engine/model.rs', '    let settings=json_col(settings)?; let transient=json_col(transient)?;', '    let settings=json_col(settings)?; let transient=json_col(transient)?;\n    crate::codec::validate_bot_times(&settings,&transient).map_err(data)?;\n    us(started.as_deref())?; us(changed.as_deref())?;', 'test(r9c_mcp_can_stop)'),
    ('r9c_stop_damaged', 'rust/src/web/bot/write.rs', 'Err(eligibility::Refusal::Unreadable(_)) if action == Action::Stop => Ok(()),', 'Err(eligibility::Refusal::Unreadable(_)) if false && action == Action::Stop => Ok(()),', 'test(r9c_mcp_can_stop) | test(r9_each_stored_time_refuses)'),

    ('r9_writer_strict_timestamp', 'rust/src/web/bot/write.rs', 'pub use crate::engine::accounting::web_pending as pending;', 'pub fn pending(c:&rusqlite::Connection,bot:&super::Bot,now:chrono::DateTime<chrono::Utc>)->Result<crate::web::format::Num,crate::web::WebError> {\n    let mut observed=bot.clone();\n    let since=bot.transient.get("quote_amount_limit_enabled_at").and_then(serde_json::Value::as_str).and_then(|s|chrono::DateTime::parse_from_rfc3339(s).ok());\n    if since.is_none() {observed.transient.remove("quote_amount_limit_enabled_at");}\n    crate::engine::accounting::web_pending(c,&observed,now)\n}', 'test(r9_mcp_settings_cancel_resume_tick)'),
    ('r9_timestamp_error_discard', 'rust/src/codec.rs', 'Some(serde_json::Value::String(text)) => parse_time(text).map(Some),', 'Some(serde_json::Value::String(text)) => Ok(parse_time(text).ok()),', 'test(r9_each_stored_time) | test(r9_garbage_timestamp)'),
    ('r9_row_time_validation', 'rust/src/figures/fill.rs', 'crate::codec::parse_time(text).map_err(|e|FiguresError::Data(format!("{e:?}")))?;', 'if false {crate::codec::parse_time(text).map_err(|e|FiguresError::Data(format!("{e:?}")))?;}', 'test(r9_each_stored_time)'),

    ('sell', 'rust/src/engine/basket.rs', 'Fill::of(order.sell, &order.kind)', 'Fill::of(false, &order.kind)', 'test(history_decisions_match)'),
    ('normalization', 'rust/src/figures/fill.rs', 'let quantity=raw.amount_exec.as_ref().or(if closed{raw.amount.as_ref()}else{None});', 'if raw.price.is_none() { return Ok(None); }\n    let quantity=raw.amount_exec.as_ref().or(if closed{raw.amount.as_ref()}else{None});', 'test(history_decisions_match)'),
    ('merge_walk', 'rust/src/engine/basket.rs', 'for order in orders {', 'for order in orders { if bot.merged_history() { continue; }', 'test(history_decisions_match)'),
    ('split_tie', 'rust/src/engine/basket.rs', 'e.at_us <= created_us', 'e.at_us < created_us', 'test(history_decisions_match)'),
    ('split_tail', 'rust/src/engine/basket.rs', 'for e in pending { apply(&mut ledger, &mut w, e)?; }', 'for _e in pending {}', 'test(history_decisions_match)'),
    ('carry_boundary', 'rust/src/figures/fill.rs', 'created_at, created_at >= ?2 FROM transactions WHERE bot_id=?1 AND status=0', 'created_at, created_at > ?2 FROM transactions WHERE bot_id=?1 AND status=0', 'test(history_decisions_match) | test(r9_stored_timestamp_shapes)'),
    ('r10_parsed_window', 'rust/src/figures/fill.rs', 'if !r.get::<_,bool>(7)? {continue;}', 'if crate::codec::parse_time(&r.get::<_,String>(6)?).map_err(|e|FiguresError::Data(format!("{e:?}")))? < crate::codec::parse_time(since).map_err(|e|FiguresError::Data(format!("{e:?}")))? {continue;}', 'test(r10_window_membership)'),
    ('normalized_credit', 'rust/src/figures/fill.rs', 'fill.map_or_else(||raw.quote_amount_exec.clone().unwrap_or_else(Dec::zero),|f|f.value)', 'raw.quote_amount_exec.clone().unwrap_or_else(Dec::zero)', 'test(history_decisions_match)'),
    ('waiting_commitment', 'rust/src/figures/fill.rs', '(None,Some(a),Some(p))=>(a*p)?,', '(None,Some(_a),Some(_p))=>fill.as_ref().map_or_else(Dec::zero,|f|f.value.clone()),', 'test(history_decisions_match)'),
    ('reservation', 'rust/src/figures/fill.rs', 'let remainder=(&requested-&filled)?;', 'let remainder=requested; let _observed=filled;', 'test(history_decisions_match)'),
    ('unknown_quantity', 'rust/src/figures/fill.rs', 'if fill.is_none() && order.raw.quote_amount_exec.as_ref().is_some_and(Dec::is_positive)', 'if false', 'test(unknown_fill_value)'),
    ('cap_credit', 'rust/src/figures/fill.rs', 'rusqlite::types::ValueRef::Null if value.is_zero()=>CapKind::Integer(0)', 'rusqlite::types::ValueRef::Null=>CapKind::Integer(0)', 'test(history_decisions_match)'),
    ('sell_lift', 'rust/src/engine/eligibility.rs', 'if !alpaca && sells > 0', 'if sells > 0', 'test(sells_and_merges_pass)'),
    ('merge_lift', 'rust/src/engine/eligibility.rs', 'if !alpaca && bot.merged_history()', 'if bot.merged_history()', 'test(sells_and_merges_pass)'),
    ('unreadable', 'rust/src/engine/eligibility.rs', 'if alpaca && r.is_empty() { super::basket::walk(c, bot, Utc::now())?; }', 'if alpaca && r.is_empty() {}', 'test(unknown_fill_value)'),
    ('active_rules', 'rust/src/engine/eligibility.rs', 'if rules > 0', 'if false', 'test(an_active_rule)'),
    ('unreadable_report', 'rust/src/engine/eligibility.rs', 'report.unreadable.push((id, format!("{e:?}")));', 'let _observed = (id,e);', 'test(unknown_fill_value)'),
    ('working_report', 'rust/src/engine/eligibility.rs', 'report.problems.push(format!("bot {id} ({}): {}", bot.status.label(), reasons.join(", ")));', 'let _observed = &reasons;', 'test(deferred_classes_and_existing_refusals)'),
    ('idle_work_report', 'rust/src/engine/eligibility.rs', 'if !work.is_empty() {', 'if false {', 'test(a_stopped_bot_outside_the_slice)'),
    ('stale_report', 'rust/src/engine/eligibility.rs', 'report.problems.push(format!("bot {id} ({}): {}", bot.status.label(), s.message));', 'let _observed = s;', 'test(check_and_the_takeover_refuse)'),
    ('problems_refusal', 'rust/src/engine/eligibility.rs', 'if !self.problems.is_empty()', 'if false', 'test(a_report_refuses_problems_first)'),
    ('unreadable_refusal', 'rust/src/engine/eligibility.rs', 'if !self.unreadable.is_empty()', 'if false', 'test(a_report_refuses_problems_first)'),
    ('guard_rules', 'rust/src/engine/eligibility.rs', 'check_install(tx).map_err(failed)?.refusal()?;', 'check_install(tx).map_err(failed)?;', 'test(unknown_fill_value)'),
    ('guard_stranded', 'rust/src/engine/eligibility.rs', 'if !stranded.is_empty()', 'if false', 'test(a_legacy_intent_gets_its_snapshot)'),
    ('guard_preflight', 'rust/src/engine/eligibility.rs', 'crate::venue::alpaca::preflight(tx, cipher).map(|_| ()).map_err(Refusal::Untradable)', 'Ok(())', 'test(the_guard_refuses_a_start_this_build_cannot_trade)'),
    ('rebalance_pending', 'rust/src/engine/eligibility.rs', '"rebalance_pending"', '"mutant_rebalance_pending"', 'test(deferred_classes_and_existing_refusals)'),
    ('liquidation_pending', 'rust/src/engine/eligibility.rs', '"liquidation_pending"', '"mutant_liquidation_pending"', 'test(deferred_classes_and_existing_refusals)'),
    ('redeploy_pending', 'rust/src/engine/eligibility.rs', '"redeploy_pending"', '"mutant_redeploy_pending"', 'test(deferred_classes_and_existing_refusals)'),
    ('r1_wire_serialization', 'rust/src/engine/amount.rs', '? <= floored { return Ok(legacy); }', '? >= BigDec::zero() { return Ok(legacy); }', 'test(every_recorded_rails_alpaca_sizing_and_wire)'),
    ('r1_zero_substitution', 'rust/src/figures/fill.rs', 'if closed && raw.amount_exec.is_none() && raw.amount.is_none() && raw.quote_amount_exec.is_none()', 'if false && closed && raw.amount_exec.is_none() && raw.amount.is_none() && raw.quote_amount_exec.is_none()', 'test(r1_null_closed)'),
    ('r1_exact_instead_of_float', 'rust/src/engine/accounting.rs', 'CapKind::RealBits(bits) => Num::Float(f64::from_bits(bits))', 'CapKind::RealBits(_bits) => Num::Dec(row.value.clone())', 'test(r1_cap_float) | test(r1_recorded_sources)'),
    ('r1_post_quantization_guard', 'rust/src/engine/accounting.rs', 'if cost <= exact { return Ok(CapGuard::Place(plan.clone())); }', 'if true { return Ok(CapGuard::Place(plan.clone())); }', 'test(r1_guard_limits)'),
    ('r1_guard_call', 'rust/src/engine/placement.rs', 'super::amount::guard_exact_cap(&tx, &current, plan)?', 'super::amount::CapGuard::Place(plan.clone())', 'test(r1_guard_limits)'),
    ('r1_merge_cutoff', 'rust/src/engine/tick.rs', '(?2 IS NULL OR id>?2)', '(?2 IS NULL OR id>0)', 'test(r1_merged_first_tick)'),
    ('r1_first_tick_only', 'rust/src/engine/tick.rs', 'if first_tick {', 'if true || first_tick {', 'test(r1_merged_first_tick)'),
    ('r1_warning_event', 'rust/src/engine/tick.rs', '(cx.below_minimum)(bot_id, rows.into_iter().map(|(id, _)| id).collect());', 'if false { (cx.below_minimum)(bot_id, rows.into_iter().map(|(id, _)| id).collect()); }', 'test(r1_merged_first_tick)'),
    ('r2_rounded_cost', 'rust/src/engine/accounting.rs', 'if cost <= exact { return Ok(CapGuard::Place(plan.clone())); }', 'if cost.floor(quote) <= exact { return Ok(CapGuard::Place(plan.clone())); }', 'test(r2_exact_cost)'),
    ('r2_no_reduction', 'rust/src/engine/accounting.rs', 'if cost <= exact { return Ok(CapGuard::Place(plan.clone())); }', 'if true { return Ok(CapGuard::Place(plan.clone())); }', 'test(r2_exact_cost)'),
    ('r2_minimum_on_unchanged', 'rust/src/engine/accounting.rs', 'if cost <= exact { return Ok(CapGuard::Place(plan.clone())); }', 'if cost <= exact { return Ok(if cost < plan.ticker.minimum_quote_size { CapGuard::BelowMinimum(plan.clone()) } else { CapGuard::Place(plan.clone()) }); }', 'test(r2_unchanged_minimum)'),
    ('r2_skip_report', 'rust/src/engine/tick.rs', 'placement::Begun::BelowMinimum(skipped) => { legs.skipped.push(*skipped); continue; }', 'placement::Begun::BelowMinimum(_skipped) => { continue; }', 'test(r2_reduced_below)'),
    ('r2_convergence', 'rust/src/engine/accounting.rs', 'if cost > exact { return Err(EngineError::Data(', 'if false { return Err(EngineError::Data(', 'test(r2_guard_refuses)'),
    ('r3_silent_discard', 'rust/src/figures/fill.rs', 'fn parse_raw(raw: &Raw, closed: bool) -> Result<Option<Fill>, FiguresError> {\n    validate_raw(raw)?;', 'fn parse_raw(raw: &Raw, closed: bool) -> Result<Option<Fill>, FiguresError> {\n    if validate_raw(raw).is_err() { return Ok(None); }', 'test(r3_negative_fill)'),
    ('r3_no_negative_validation', 'rust/src/figures/fill.rs', 'if value.is_negative() {', 'if false && value.is_negative() {', 'test(r3_negative_fill)'),
    ('r3_eligibility_bypass', 'rust/src/engine/eligibility.rs', '    model::merged_history_cutoff(c, bot)?;', '    // mutant bypasses cutoff validation', 'test(r3_malformed_merge)'),
    ('r3_string_boundary', 'rust/src/engine/model.rs', "s.bytes().fold(0_i64, |n,b| n.saturating_mul(10).saturating_add(i64::from(b - b'0')))", '0_i64', 'test(r3_integer_and_ascii)'),
    ('r3_null_boundary', 'rust/src/engine/model.rs', 'if rows { return Err(invalid()); }', 'if false && rows { return Err(invalid()); }', 'test(r3_malformed_merge)'),
    ('r4_tick_cutoff_validation', 'rust/src/engine/model.rs', 'let cutoff = match bot.transient.get("merged_history_until_id") {', 'let cutoff = match None::<&Value> {', 'test(r3_malformed_merge)'),
    ('r4_tick_negative_validation', 'rust/src/figures/fill.rs', 'fn parse_raw(raw: &Raw, closed: bool) -> Result<Option<Fill>, FiguresError> {\n    validate_raw(raw)?;', 'fn parse_raw(raw: &Raw, closed: bool) -> Result<Option<Fill>, FiguresError> {\n    if validate_raw(raw).is_err() { return Ok(None); }', 'test(r3_negative_fill)'),
    ('r4_tick_null_validation', 'rust/src/figures/fill.rs', 'if closed && raw.amount_exec.is_none() && raw.amount.is_none() && raw.quote_amount_exec.is_none()', 'if false && closed && raw.amount_exec.is_none() && raw.amount.is_none() && raw.quote_amount_exec.is_none()', 'test(r1_null_closed)'),
    ('r5_raw_writer', 'rust/src/engine/accounting.rs', 'let rows = crate::figures::fill::action_commitments(c, bot.id, &codec::format_time(since)).map_err(web_bot::fill_error)?;', 'let rows = raw_commitments(c, bot.id, &codec::format_time(since))?;\n\nfn raw_commitments(c: &Connection, id: i64, since: &str) -> Result<Vec<Commitment>, WebError> {\n    let mut q=c.prepare("SELECT external_status,quote_amount_exec FROM transactions WHERE bot_id=?1 AND status=0 AND side=0 AND transaction_type=\'REGULAR\' AND created_at>=?2 ORDER BY id")?;\n    let rows=q.query_map((id,since),|row|Ok((row.get::<_,i64>(0)?,row.get::<_,Option<f64>>(1)?)))?;\n    let mut out=vec![];\n    for row in rows { let (status,value)=row?; let value=BigDec::from_f64(value.unwrap_or(0.0)).map_err(data)?; out.push(Commitment{status,value,cap_kind:crate::engine::accounting::CapKind::Decimal,zero_without_report:false}); }\n    Ok(out)\n}\n', 'test(r5_web_settings) | test(r5_mcp_settings)'),
    ('r5_broad_compaction', 'rust/src/engine/model.rs', 'pub fn merge_transient_compact(c: &Connection, bot_id: i64, pairs: &[(&str, Value)]) -> Result<(), EngineError> {\n    locked(c, |c| {\n        let (expr, keys) = set_keys(pairs, 2);\n        let mut args = vec![rusqlite::types::Value::Integer(bot_id)];\n        args.extend(keys);\n        let n = c.execute(&format!("UPDATE bots SET transient_data = {expr} WHERE id = ?1 AND json_type(transient_data) = \'object\'"),\n                          rusqlite::params_from_iter(args))?;\n        if n == 0 { return Err(not_an_object(bot_id)); }\n        for (key, value) in pairs {\n            if !value.is_null() { continue; }\n            let path = format!("$.\\"{key}\\"");\n            c.execute("UPDATE bots SET transient_data = json_remove(transient_data, ?2) WHERE id = ?1 AND json_type(transient_data, ?2) = \'null\'",\n                      params![bot_id, path])?;\n        }\n        Ok(())\n    })\n}\n', 'pub fn merge_transient_compact(c: &Connection, bot_id: i64, pairs: &[(&str, Value)]) -> Result<(), EngineError> {\n    locked(c, |c| {\n        let (expr, keys) = set_keys(pairs, 2);\n        let mut args = vec![rusqlite::types::Value::Integer(bot_id)];\n        args.extend(keys);\n        let n = c.execute(&format!("UPDATE bots SET transient_data = {expr} WHERE id = ?1 AND json_type(transient_data) = \'object\'"),\n                          rusqlite::params_from_iter(args))?;\n        if n == 0 { return Err(not_an_object(bot_id)); }\n        let mut s = c.prepare("SELECT e.fullkey FROM bots, json_each(bots.transient_data) AS e WHERE bots.id = ?1 AND e.type = \'null\'")?;\n        let nulls = s.query_map([bot_id], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;\n        for path in nulls {\n            c.execute("UPDATE bots SET transient_data = json_remove(transient_data, ?2) WHERE id = ?1 AND json_type(transient_data, ?2) = \'null\'",\n                      params![bot_id, path])?;\n        }\n        Ok(())\n    })\n}\n', 'test(r3_malformed_merge) | test(r5_failure_compaction)'),
    ('r5_raw_start_cap', 'rust/src/engine/accounting.rs', 'Some(since) => crate::figures::fill::action_commitments(c, bot.id, &since).map_err(web_bot::fill_error)?,', 'Some(since) => {\n\nfn raw_commitments(c: &Connection, id: i64, since: &str) -> Result<Vec<Commitment>, WebError> {\n    let mut q=c.prepare("SELECT external_status,quote_amount_exec FROM transactions WHERE bot_id=?1 AND status=0 AND side=0 AND transaction_type=\'REGULAR\' AND created_at>=?2 ORDER BY id")?;\n    let rows=q.query_map((id,since),|row|Ok((row.get::<_,i64>(0)?,row.get::<_,Option<f64>>(1)?)))?;\n    let mut out=vec![];\n    for row in rows { let (status,value)=row?; let value=BigDec::from_f64(value.unwrap_or(0.0)).map_err(data)?; out.push(Commitment{status,value,cap_kind:crate::engine::accounting::CapKind::Decimal,zero_without_report:false}); }\n    Ok(out)\n}\n\nraw_commitments(c, bot.id, &since)? },', 'test(r5_start_cap)'),
    ('r5_raw_sell_cap', 'rust/src/engine/accounting.rs', 'for amount in crate::figures::fill::sold_commitments(c, id, since).map_err(web_bot::fill_error)? {', 'for amount in { let mut q=c.prepare("SELECT COALESCE(amount_exec,amount,0) FROM transactions WHERE bot_id=?1 AND side=1 AND created_at>=?2")?; let rows=q.query_map((id,since),|r|r.get::<_,f64>(0))?; let mut values=vec![]; for v in rows { values.push(SoldCommitment::normalized(BigDec::from_f64(v?).map_err(data)?)); } values } {', 'test(r5_draft_sell)'),
    ('r5_settings_scope', 'rust/src/web/bot/write.rs', 'let original_refusal = if submitted.mcp() { None } else { super::refusal(&tx, id, wash_sale, provider, For::Settings)? };', 'let original_refusal = if submitted.mcp() { None } else { super::refusal(&tx, id, wash_sale, provider, For::Page)? };', 'test(r5_web_settings)'),
    ('r5_settings_after_scope', 'rust/src/web/bot/write.rs', 'if !submitted.mcp() { if let Some(reason) = super::refusal(&tx, id, wash_sale, provider, For::Settings)?', 'if !submitted.mcp() { if let Some(reason) = super::refusal(&tx, id, wash_sale, provider, For::Page)?', 'test(r5_web_settings)'),
    ('r6_sequential_exact_bucket', 'rust/src/engine/accounting.rs', 'let mut spent = ruby_sum(&closed).map_err(EngineError::Arithmetic)?.add(&ruby_sum(&waiting).map_err(EngineError::Arithmetic)?).map_err(EngineError::Arithmetic)?\n            .add(&ruby_sum(&stopped).map_err(EngineError::Arithmetic)?).map_err(EngineError::Arithmetic)?;', 'let mut spent = Num::Dec(rows.iter().try_fold(BigDec::zero(), |sum, row| sum.checked_add(&row.value)).map_err(data)?);', 'test(r6_)'),
    ('r6_start_scope', 'rust/src/web/bot/write.rs', 'super::refusal(&tx,id,wash,provider,For::Settings)?', 'super::refusal(&tx,id,wash,provider,For::Page)?', 'test(r6_web_start_action)'),
    ('r7_unchecked_sum', 'rust/src/ruby.rs', 'n = n.checked_add(*x).ok_or(CodecError::IntegerOverflow("ruby_sum"))?;', 'n = n.wrapping_add(*x);', 'test(r7_integer_sum) | test(r7_integer_cap)'),
    ('r7_unchecked_add', 'rust/src/ruby.rs', 'a.checked_add(*b).ok_or(CodecError::IntegerOverflow("Num::add"))?', 'a.wrapping_add(*b)', 'test(r7_integer_add)'),
    ('r7_unchecked_sub', 'rust/src/ruby.rs', 'a.checked_sub(*b).ok_or(CodecError::IntegerOverflow("Num::sub"))?', 'a.wrapping_sub(*b)', 'test(r7_integer_sub)'),
    ('r7_unchecked_multiply', 'rust/src/engine/accounting.rs', 'n.checked_mul(count).ok_or(EngineError::Arithmetic(codec::CodecError::IntegerOverflow("pending multiply")))?', 'n.wrapping_mul(count)', 'test(r7_integer_pending)'),
    ('r7_check_bypass', 'rust/src/engine/eligibility.rs', 'if alpaca && r.is_empty() { super::accounting::validate_stored_amounts(&bot.settings, &bot.transient)?; super::accounting::quote_amount_available_num(c, bot)?; }', 'if alpaca && r.is_empty() {}', 'test(r8_settings_bound)'),
    ('r8_integer_float_adapter', 'rust/src/engine/accounting.rs', 'pending_window(bot.effective_quote_amount_num()?, intervals, carry_num(bot.transient.get("missed_quote_amount"))?, &rows)?', 'pending_window(Num::Float(smart.unwrap_or(quote_amount)), intervals, carry_num(bot.transient.get("missed_quote_amount"))?, &rows)?', 'test(r8_integer_engine)'),
    ('r8_magnitude_bound', 'rust/src/engine/accounting.rs', 'if value > &bound || value < &(&BigDec::zero() - &bound)', 'if false && (value > &bound || value < &(&BigDec::zero() - &bound))', 'test(r8_)'),
    ('r8_normalized_cost_bound', 'rust/src/figures/fill.rs', 'crate::engine::accounting::bounded_fill_amount(value).map_err(|_|FiguresError::Data("accounting magnitude exceeds 2^53".into()))', 'Ok(())', 'test(r8_fill_cost)'),
    ('r8_fill_bound_serialization', 'rust/src/engine/accounting.rs', 'if magnitude > crate::figures::dec::Dec::from_i64(9_007_199_254_740_992)', 'if false && magnitude > crate::figures::dec::Dec::from_i64(9_007_199_254_740_992)', 'test(r8_fill_cost_boundary)'),
    ('r8_submitted_bound', 'rust/src/web/bot/action_params.rs', 'numeric_with_bound(value, false, true)', 'numeric_with_bound(value, false, false)', 'test(r8_submitted)'),
    ('upstream_start_key', 'rust/src/web/bot/draft.rs', 'if key != Some(crate::enums::ApiKeyStatus::Correct as i64)', 'if false && key != Some(crate::enums::ApiKeyStatus::Correct as i64)', 'test(action_draft_rails_vectors)'),
    ('r8_mcp_paper_gate', 'rust/src/web/mcp/tools.rs', 'else if LIVE_AUTOMATION_TOOLS.contains(&name) && paper_trading(c,who.user_id)?', 'else if false && LIVE_AUTOMATION_TOOLS.contains(&name) && paper_trading(c,who.user_id)?', 'test(r8_merged_mcp)'),
]
for args in new: test(*args)

def private_bucket(name, helper):
    if len(sys.argv)>1 and name not in sys.argv[1:]: return
    started = time.time()
    path=root/'rust/src/web/bot/write.rs';original=path.read_text()
    try:
        path.write_text(original+'\n'+helper+'\n')
        # The Python tripwire deliberately accepts this: rustc privacy is the gate.
        disk(name)
        with (logs/f'{name}.log').open('w') as log:
            tripwire=run_mutant(name,[sys.executable,str(root/'script/rust/histories_accounting_gate.py')],log,started)
            assert tripwire.returncode == 0, f'{name}: unexpected tripwire failure'
            result=run_mutant(name,['cargo','check','--locked'],log,started)
        output=(logs/f'{name}.log').read_text()
        assert result.returncode==101 and 'error[E0616]' in output and 'field `value` of struct `Commitment` is private' in output, output
        print(f'KILLED {name} by rustc privacy build gate',flush=True)
    finally:path.write_text(original)
private_bucket('r6_duplicate_web_bucket', 'fn second_cap_sum(rows: &[crate::figures::fill::Commitment]) -> crate::ruby::BigDec { rows.iter().fold(crate::ruby::BigDec::zero(), |total,row| &total + &row.value) }')
private_bucket('r7_duplicate_web_loop', 'fn duplicate_cap(limit: &crate::ruby::BigDec, rows: &[crate::figures::fill::Commitment]) -> crate::ruby::BigDec { let mut total = crate::ruby::BigDec::zero(); for row in rows { total = &total + &row.value; } (limit - &total).max(crate::ruby::BigDec::zero()) }')
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
    if 'quote_amount_limit_enabled_at_us()' in old:
        # R9f: validate_times rejects malformed timestamps before the later redundant
        # r.push(e). Remove the active shared refusal, not an unreachable duplicate.
        # The original duplicate-only survivor is retained in the draft evidence.
        test(f'refusal_{number:02}', 'rust/src/codec.rs',
             'Some(_) => Err(CodecError::Time("unreadable stored timestamp".into())),',
             'Some(_) => Ok(None),', 'test(every_remaining_eligibility_branch)')
    else:
        test(f'refusal_{number:02}',relative,old,replacement,
             'test(every_remaining_eligibility_branch) | test(deferred_classes_and_existing_refusals)')
print(f'PASS selected mutations' if len(sys.argv)>1 else f'PASS {len(new)} behavior/global mutations and {len(starts)} per-branch refusal mutations',flush=True)
