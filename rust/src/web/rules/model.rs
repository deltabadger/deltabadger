//! Rules::Withdrawal's page data and calculations. No evaluation or withdrawal runs here.
use crate::{
    ruby::BigDec,
    web::{bot::Asset, wizard::keys::Venue, App, WebError},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

#[derive(Clone)]
pub struct Rule {
    pub id: i64,
    pub user_id: i64,
    pub status: i64,
    pub exchange: Venue,
    pub asset: Asset,
    pub address: String,
    pub settings: Value,
    pub fee: Option<BigDec>,
    pub chains: Vec<Value>,
    pub balance: Option<BigDec>,
    pub unconfirmed: Option<String>,
}

pub fn string(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}
impl Rule {
    pub fn text(&self, key: &str) -> String {
        string(&self.settings[key])
    }
    pub fn value(&self, key: &str, default: &str) -> String {
        self.settings
            .get(key)
            .filter(|v| !v.is_null())
            .map(string)
            .unwrap_or_else(|| default.into())
    }
    pub fn percentage(&self) -> String {
        let p = self.text("withdrawal_percentage");
        if crate::ruby::blank(&p) {
            "100".into()
        } else {
            p
        }
    }
    pub fn threshold(&self) -> String {
        self.value(
            "threshold_type",
            if self.fee.is_some() {
                "fee_percentage"
            } else {
                "min_amount"
            },
        )
    }
    pub fn fee_amount(&self) -> BigDec {
        self.chains
            .iter()
            .find(|c| c["name"].as_str() == Some(self.text("network").as_str()))
            .and_then(|c| crate::ruby::json_to_d(&c["fee"]).ok().flatten())
            .or_else(|| self.fee.clone())
            .unwrap_or_else(BigDec::zero)
    }
    pub fn minimum(&self) -> Option<BigDec> {
        let pct = BigDec::parse(&self.percentage()).ok()?;
        if !pct.is_positive() {
            return None;
        }
        let fraction = pct.div(&BigDec::from_i64(100))?;
        if self.text("threshold_type") == "min_amount" {
            Some(
                BigDec::parse(&self.text("min_amount"))
                    .ok()?
                    .div(&fraction)?
                    .round(8),
            )
        } else {
            let max = BigDec::parse(&self.text("max_fee_percentage")).ok()?;
            let fee = self.fee_amount();
            if max.is_zero() || fee.is_zero() {
                return None;
            }
            Some(
                fee.div(&max.div(&BigDec::from_i64(100))?)?
                    .div(&fraction)?
                    .round(8),
            )
        }
    }
}

pub fn load(c: &Connection, app: &App, user: i64, id: i64) -> Result<Option<Rule>, WebError> {
    type Row = (i64, i64, i64, Option<String>, String);
    let row: Option<Row> = c.query_row("SELECT status, exchange_id, asset_id, address, settings FROM rules WHERE id=?1 AND user_id=?2 AND type='Rules::Withdrawal'",
        params![id,user], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    let Some((status, exchange, asset, address, settings)) = row else {
        return Ok(None);
    };
    let Some(exchange) = crate::web::wizard::keys::venue(c, Some(exchange))? else {
        return Ok(None);
    };
    let Some(asset) = Asset::find(c, asset)? else {
        return Ok(None);
    };
    let address = address
        .map(|a| app.cipher.decrypt(&a))
        .transpose()
        .map_err(|_| WebError::Config("rule address unreadable".into()))?
        .unwrap_or_default();
    let (fee, chains): (Option<String>, Option<String>) = c.query_row("SELECT CAST(withdrawal_fee AS TEXT), withdrawal_chains FROM exchange_assets WHERE exchange_id=?1 AND asset_id=?2 LIMIT 1",
        params![exchange.id,asset.id], |r| Ok((r.get(0)?,r.get(1)?))).optional()?.unwrap_or_default();
    let log: Option<(i64, Option<String>, String)> = c.query_row("SELECT status,message,details FROM rule_logs WHERE rule_id=?1 ORDER BY created_at DESC LIMIT 1", [id],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let balance = log.and_then(|(status, message, details)| {
        if status == 1 {
            return Some(BigDec::zero());
        }
        let details: Value = serde_json::from_str(&details).unwrap_or_default();
        if let Some(value) = details["free_balance"]
            .as_str()
            .and_then(|v| BigDec::parse(v).ok())
        {
            return Some(value);
        }
        message?
            .strip_prefix("Balance ")?
            .split_whitespace()
            .next()
            .and_then(|s| BigDec::parse(s).ok())
    });
    // W2 introduces this table. W1 reads it when present without creating engine state.
    let has_intents: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='rust_withdrawal_intents')", [], |r| r.get(0))?;
    let unconfirmed = if has_intents {
        c.query_row("SELECT state FROM rust_withdrawal_intents WHERE owner=?1 AND state IN ('dispatched','unresolved')",
            [format!("rule:{id}")], |r| r.get::<_,String>(0)).optional()?
    } else {
        None
    };
    Ok(Some(Rule {
        id,
        user_id: user,
        status,
        exchange,
        asset,
        address,
        settings: serde_json::from_str(&settings).unwrap_or_else(|_| json!({})),
        fee: fee.and_then(|s| BigDec::parse(&s).ok()),
        chains: chains
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        balance,
        unconfirmed,
    }))
}
