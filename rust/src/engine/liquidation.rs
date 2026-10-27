//! Bot::LiquidateExitedJob and Composition::Liquidatable: one-shot, user-named market sales.
use super::{amount::{self, TxKind}, composition, model::{self, Level}, placement, rebalance, tick, Clock, EngineError};
use crate::{enums::BotStatus, ruby::BigDec, venue::{Attributed, Side, Venue}};
use rusqlite::Connection;
use serde_json::{json, Value};

pub const REQUEST_KEY: &str = "liquidation_selling_since";

pub(crate) fn refuse(c: &Connection, id: i64, reason: Value, clock: &dyn Clock) -> Result<(), EngineError> {
    model::log_activity(c, id, "liquidation_not_started", Level::Info, json!({"reason":reason}), clock.now())
}
fn skip(c: &Connection, id: i64, base: &str, reason: &str, clock: &dyn Clock) -> Result<(), EngineError> {
    model::log_activity(c, id, "liquidation_skipped", Level::Info, json!({"base":base,"reason":reason}), clock.now())
}
fn named(holdings: &Value) -> Result<Vec<(String, Option<i64>)>, EngineError> {
    let mut names = vec![];
    for entry in holdings.as_array().ok_or_else(|| EngineError::Data("liquidation holdings must be an array".into()))? {
        let key = entry[0].as_str().ok_or_else(|| EngineError::Data("liquidation holding must name its key".into()))?;
        let expected = match &entry[1] {
            Value::Null => None,
            v => Some(v.as_i64().or_else(|| v.as_str()?.parse().ok()).ok_or_else(|| EngineError::Data("liquidation asset id is unreadable".into()))?),
        };
        if !names.iter().any(|(k, _)| k == key) { names.push((key.to_string(), expected)); }
    }
    Ok(names)
}

/// The request's owner clears only the version it took, including all refusal and error exits.
pub(crate) fn clear(c: &Connection, id: i64, request: &Value) -> Result<(), EngineError> {
    model::locked(c, |c| {
        if model::load_bot(c,id)?.transient[REQUEST_KEY] == *request {
            model::merge_transient_compact(c,id,&[(REQUEST_KEY,Value::Null)])?;
        }
        Ok(())
    })
}

pub async fn run<V: Venue>(c: &Connection, venue: &V, id: i64, holdings: &Value, clock: &dyn Clock, cx: &tick::TickContext<'_>) -> Result<(), EngineError> {
    let handle = crate::venue::Handle::new(venue,cx.credential_version.clone());
    let venue = &handle;
    let bot = model::load_bot(c,id)?;
    if matches!(bot.status,BotStatus::Deleted | BotStatus::Archived) { return refuse(c,id,json!("archived"),clock); }
    if model::key_pending(c,&bot)? { return refuse(c,id,json!("api_key_pending"),clock); }
    let names = named(holdings)?;
    let mut claim = rebalance::Claim { composition: placement::composition_snapshot(c,&bot)?, bot };
    // Market first, using only the named assets. The ledger resolves legacy names without a price read.
    if venue.rules().market_hours {
        let subject = crate::figures::db::Subject::load(c,id).map_err(data)?;
        let at = crate::figures::at::At::from_utc(clock.now()).ok_or_else(|| data("invalid time"))?;
        let walked = crate::figures::walk::metrics(c,&subject,at).map_err(data)?;
        let mut tickers = vec![];
        for (key,expected) in &names {
            let asset = expected.or_else(|| walked.key_assets.iter().find(|(k,_)|k==key).and_then(|(_,a)|*a));
            if let Some(asset) = asset {
                if let Some(ticker) = model::ticker_for_asset(c,&claim.bot,asset)? { tickers.push(ticker); }
            }
        }
        let crypto = if tickers.is_empty() { model::all_crypto(c,&claim.bot)? } else { tickers.iter().all(|t|t.crypto) };
        if !crypto && !matches!(super::clock::gate(venue.clock_result().await.value,&model::exchange_name(c,&claim.bot)?,clock.now()),super::clock::Gate::Open) {
            return model::log_activity(c,id,"liquidation_market_closed",Level::Info,json!({}),clock.now());
        }
    }
    let waiting: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id=?1 AND side=1 AND status=0 AND external_status IN (0,1))",[id],|r|r.get(0))?;
    if waiting {
        let watched = composition::waiting_rows(c,Some(id),None)?;
        let before = composition::halted(c,&watched)?;
        if let Err(e) = super::polling::sweep(c,venue,&claim.bot,clock.now()).await { super::log(&format!("[engine] liquidation sweep bot={id}: {e:?}")); }
        composition::halt_abandoned(c,&watched,&before,clock.now())?;
    }
    composition::promote_orphan(c,id,TxKind::Liquidation,clock.now())?;
    let current = model::load_bot(c,id)?;
    let reason = if rebalance::is_pending(&current.transient) { Some("rebalance_pending") }
        else if current.transient["liquidation_pending"].as_object().is_some_and(|p|!p.is_empty()) { Some("halted") }
        else if c.query_row("SELECT EXISTS(SELECT 1 FROM transactions WHERE bot_id=?1 AND side=1 AND status=0 AND external_status IN (0,1))",[id],|r|r.get::<_,bool>(0))? { Some("orders_waiting") }
        else if composition::unresolved_liquidations(c,&current)? { Some("orders_unresolved") }
        else if composition::redeploy_in_flight(c,&current)? { Some("redeploy_pending") }
        else if current.rust_placement().is_some() { Some("halted") }
        else if super::reversal::barred(&current) { Some("reversal_pending") }
        else { None };
    if let Some(reason) = reason { return refuse(c,id,json!([reason]),clock); }
    if let Some(stale) = super::staleness::stale(c,&claim.bot,clock.now())? {
        return refuse(c,id,json!([stale.message]),clock);
    }
    if !rebalance::before_rebalance(c,venue,&mut claim,clock,cx).await? { return refuse(c,id,json!(["changed"]),clock); }
    let Some(reconciled) = rebalance::split_gate(c,venue,&claim,clock).await? else { return refuse(c,id,json!(["split_hold"]),clock) };
    let (walked,live) = match rebalance::holdings(c,venue,&claim.bot,clock.now()).await? {
        Ok(data) => data,
        Err(rebalance::ValueFail::Transient(m) | rebalance::ValueFail::Raised(m)) => return Err(EngineError::Data(m)),
    };
    let mut selected = vec![];
    for (key,expected) in &names {
        let Some(asset) = walked.key_assets.iter().find(|(k,_)|k==key).and_then(|(_,a)|*a) else { continue };
        if expected.is_some_and(|a|a!=asset) { continue; }
        let Some(ticker) = model::ticker_for_asset(c,&claim.bot,asset)? else { continue };
        let Some(value) = live.as_ref().and_then(|d|d.asset_values.iter().find(|(k,_)|k==key)).map(|(_,v)|v) else { continue };
        let amount = BigDec::parse(&value.amount.to_d().map_err(data)?.to_s_f()).map_err(data)?;
        if amount < ticker.minimum_base_size { continue; }
        selected.push((key.clone(),ticker));
    }
    if selected.is_empty() { return refuse(c,id,json!(["not_held"]),clock); }
    for (key,_) in &names {
        if !selected.iter().any(|(k,_)|k==key) { skip(c,id,key,"not_held",clock)?; }
    }
    for (index,(key,ticker)) in selected.iter().enumerate() {
        let interrupted = || model::log_activity(c,id,"liquidation_batch_cut_short",Level::Info,
            json!({"bases":selected[index..].iter().map(|(key,_)|key.as_str()).collect::<Vec<_>>().join(", ")}),clock.now());
        if (cx.stopping)() { interrupted()?; break; }
        if !ticker.available || !ticker.trading_enabled { skip(c,id,key,"unavailable",clock)?; continue; }
        if rebalance::account_waiting_buys(c,&claim.bot,ticker)? { skip(c,id,key,"open_order",clock)?; continue; }
        let Some(price) = tick::market_price_for(venue,&claim.bot,ticker,Side::Sell,clock,cx).await? else { skip(c,id,key,"unpriced",clock)?; continue };
        let held = walked.asset_breakdown.iter().find(|(k,_)|k==key).map(|(_,v)|&v.amount).ok_or_else(||data("holding disappeared"))?;
        let held = BigDec::parse(&held.to_d().map_err(data)?.to_s_f()).map_err(data)?;
        let free = tick::live_free_balance(c,venue,&claim.bot,ticker,ticker.base_asset_id).await?.map_err(data)?;
        let amount = held.min(free);
        if !amount.is_positive() { skip(c,id,key,"unpriced",clock)?; continue; }
        let quote = &amount * &price;
        let (plan,below) = amount::size_order(ticker,Side::Sell,&price,&amount,&quote,venue.rules().minimum_logic,TxKind::Liquidation)?;
        let plan = plan.with_wire_price(venue.rules(),&price,true)?;
        if below { skip(c,id,key,"below_minimum",clock)?; continue; }
        let loss = super::wash::sell_at_loss(c,&claim.bot,ticker,&amount,&quote,clock.now())?;
        let hold = venue.placement_hold(ticker,Side::Sell).await.map_err(data)?;
        let begun = placement::begin_operation(c,&claim.bot,&plan,&[ticker],&claim.composition,&reconciled,&cx.credential_version,venue.placement_identity(),
            placement::Op::Liquidation { symbol: &ticker.base_code },loss,hold.as_ref(),clock);
        drop(hold);
        let intent = match begun? {
            placement::Begun::Intent(i) => i,
            placement::Begun::BelowMinimum(_) => { skip(c,id,key,"below_minimum",clock)?; continue; }
            placement::Begun::CredentialsChanged => return Err(EngineError::CredentialsChanged),
            placement::Begun::Refused(m) => return Err(EngineError::Data(m)),
            placement::Begun::LedgerStale(stale) => return refuse(c,id,json!([stale.message]),clock),
            placement::Begun::Changed | placement::Begun::Fenced | placement::Begun::WashSaleLocked => { interrupted()?; break; }
        };
        match placement::send_recorded(c,venue,&claim.bot,&intent,clock).await? {
            placement::Sent::Accepted(_) | placement::Sent::NotSent(_) => {}
            placement::Sent::Rejected(errors) if placement::safe_rejection(&errors) => {}
            placement::Sent::Rejected(_) | placement::Sent::BelowMinimum(_) | placement::Sent::Ambiguous(_) | placement::Sent::Fenced => break,
        }
    }
    Ok(())
}
fn data(e: impl std::fmt::Debug) -> EngineError { EngineError::Data(format!("{e:?}")) }

/// T4 / D-6C-2: attest only to the displayed nonce and abandoned rows. The same IMMEDIATE lock
/// as recovery makes its intent removal and this one exclusive. Returns the waiting venue id.
pub fn resolve(c: &Connection, id: i64, user: i64, nonce: &str, shown: &[i64], now: chrono::DateTime<chrono::Utc>) -> Result<Option<String>, EngineError> {
    use rusqlite::OptionalExtension;
    let tx=model::immediate(c)?;
    let bot=model::load_bot(&tx,id)?;
    if bot.user_id!=user { return Err(data("liquidation owner changed")); }
    let waiting:Option<Option<String>>=tx.query_row("SELECT external_id FROM transactions WHERE bot_id=?1 AND transaction_type='LIQUIDATION' AND status=0 AND external_status IN (0,1) ORDER BY id LIMIT 1",[id],|r|r.get(0)).optional()?;
    if let Some(order)=waiting { return Ok(Some(order.unwrap_or_default())); }
    let mut resolved:Vec<i64>=bot.transient["liquidation_resolved_orders"].as_array().into_iter().flatten().filter_map(|v|v.as_i64().or_else(||v.as_str()?.parse().ok())).collect();
    let rows=tx.prepare("SELECT id,base FROM transactions WHERE bot_id=?1 AND transaction_type='LIQUIDATION' AND external_status=4 ORDER BY id")?
        .query_map([id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
    let mut accounted=vec![];
    let mut changed=false;
    for (row,base) in rows {
        if shown.contains(&row) && !resolved.contains(&row) {
            resolved.push(row); changed=true;
            if let Some(base)=base { accounted.push(base); }
        }
    }
    // r5 row 2: row accounting is independent of whether a pending marker exists or matches.
    if changed { model::merge_transient_compact(&tx,id,&[("liquidation_resolved_orders",json!(resolved))])?; }
    let pending=&bot.transient["liquidation_pending"];
    if pending["state"]=="ambiguous" && pending["id"].as_str()==Some(nonce) {
        let intent=placement::load_intent(&tx,&bot,None)?;
        // A different intent must never be removed by an old page, even if a legacy marker remained.
        let matches=intent.as_ref().is_none_or(|i|i.plan.kind==TxKind::Liquidation && i.cl_ord_id==nonce);
        if matches {
            if let Some(intent)=intent {
                super::wash::settle_claim(&tx,id,super::wash::Holder::Liquidation(intent.wash_claim.as_ref()),super::wash::Reason::Ambiguous,now)?;
                model::merge_transient_compact(&tx,id,&[("rust_placement",Value::Null)])?;
            }
            model::merge_transient_compact(&tx,id,&[("liquidation_pending",Value::Null)])?;
            if let Some(base)=pending["symbol"].as_str() { accounted.insert(0,base.into()); }
            changed=true;
        }
    }
    if changed {
        let mut names=vec![];
        for name in accounted { if !name.trim().is_empty() && !names.contains(&name) { names.push(name); } }
        model::log_activity(&tx,id,"liquidation_manually_resolved",Level::Info,json!({"user_id":user,"base":if names.is_empty(){Value::Null}else{json!(names.join(", "))}}),now)?;
    }
    tx.commit()?;
    Ok(None)
}
