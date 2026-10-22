//! D-CB-SIZE-IN-QUOTE stored-row repair. Called inside takeover's ownership-claim transaction,
//! like placement::backfill_snapshots and staleness::seed; completion is an app_configs marker.
use super::activities::Raw;
use crate::{app_config, engine::EngineError};
use chrono::{DateTime, Utc};
use rusqlite::{params, types::Value, Transaction};

fn decimal(value: &Value) -> Result<Option<crate::ruby::BigDec>, EngineError> {
    crate::ruby::from_sql(value.into())
        .map_err(|e| EngineError::Data(format!("Coinbase repair: unreadable stored amount: {e:?}")))
}
pub const FLAG: &str = "rust_coinbase_size_in_quote_repaired_at";
pub fn run(tx: &Transaction<'_>, now: DateTime<Utc>) -> Result<usize, EngineError> {
    if app_config::get_plain(tx, FLAG)
        .map_err(EngineError::Data)?
        .is_some()
    {
        return Ok(0);
    }
    let mut s=tx.prepare("SELECT t.id,t.user_id,t.base_currency,t.quote_currency,t.base_amount,t.quote_amount,t.raw_data FROM account_transactions t JOIN exchanges e ON e.id=t.exchange_id WHERE e.type='Exchanges::Coinbase' AND json_valid(t.raw_data) AND json_type(t.raw_data,'$.size_in_quote')='true' ORDER BY t.id")?;
    let rows = s
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Value>(4)?,
                r.get::<_, Value>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(s);
    let mut count = 0;
    let mut owners = std::collections::BTreeSet::new();
    for (id, owner, currency, quote, old_base, old_quote, text) in rows {
        let raw = Raw::parse(&text)
            .and_then(Result::ok)
            .ok_or_else(|| EngineError::Data("Coinbase repair: unreadable raw_data".into()))?;
        let (base, total) =
            super::coinbase_ledger::amounts(&raw).map_err(|(m, _)| EngineError::Data(m))?;
        // Crypto pairs have two swap rows: the base leg holds size/price, the quote leg size; neither has quote_amount.
        let product = raw.value["product_id"].as_str().unwrap_or("");
        let mut parts = product.split('-');
        let base_currency = parts.next().unwrap_or("");
        let quote_currency = parts.next().unwrap_or("");
        let (base, quote) = if quote.is_some() {
            (base, Some(total))
        } else if currency == base_currency {
            (base, None)
        } else if currency == quote_currency {
            (total, None)
        } else {
            return Err(EngineError::Data(
                "Coinbase repair: fill currency does not match its stored row".into(),
            ));
        };
        let base = super::ledger::amount_sql(crate::venue::coinbase::COINBASE, &base);
        let quote = quote
            .as_ref()
            .map(|d| super::ledger::amount_sql(crate::venue::coinbase::COINBASE, d));
        // Read both sides through the ledger's decimal codec. NUMERIC affinity can shorten a
        // repeating quotient, so compare with what a fresh import actually stores in SQLite.
        // The Rails-compatible decimal reader keeps 16 significant digits for REAL columns.
        // Also require SQLite equality so it cannot hide a distinct stored value's final digit.
        let (stored_base, stored_quote, identical): (Value, Value, bool) = tx.query_row(
            "SELECT CAST(?1 AS NUMERIC), CAST(?2 AS NUMERIC), base_amount IS CAST(?1 AS NUMERIC) AND quote_amount IS CAST(?2 AS NUMERIC) FROM account_transactions WHERE id=?3",
            params![base, quote, id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        if decimal(&old_base)? == decimal(&stored_base)?
            && decimal(&old_quote)? == decimal(&stored_quote)?
            && identical
        {
            continue;
        }
        tx.execute("UPDATE account_transactions SET base_amount=?1,quote_amount=?2,updated_at=?3 WHERE id=?4",params![base,quote,super::sql_time(now),id])?;
        owners.insert(owner);
        count += 1;
    }
    for owner in owners {
        // Explicit invalidation also handles a future-dated row masking max(updated_at). The usual jobs rebuild both.
        for key in [
            crate::tracker::cache::key(owner),
            crate::tracker::backfill::history_key(owner),
            crate::tracker::backfill::price_key(owner),
        ] {
            tx.execute("DELETE FROM app_configs WHERE key=?1", [key])?;
        }
        tx.execute("DELETE FROM portfolio_snapshots WHERE user_id=?1", [owner])?;
        tx.execute(
            "DELETE FROM portfolio_venue_snapshots WHERE user_id=?1",
            [owner],
        )?;
    }
    app_config::set_plain(tx, FLAG, &now.to_rfc3339(), now).map_err(EngineError::Data)?;
    Ok(count)
}
