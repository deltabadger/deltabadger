//! W2: one withdrawal-rule matrix for web and future REST/MCP adapters. No sends.
//! D-RULE-ONE-MATRIX: validate every write, re-list destinations, edit/delete only
//! created or stopped rules, and never silently switch an explicitly selected threshold.
pub use super::reads::Identity;
use super::{
    model::{self, Rule},
    reads,
};
use crate::{
    ruby::BigDec,
    web::{App, WebError},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

#[derive(Clone, Debug, Default)]
pub struct Patch {
    pub settings: Value,
    pub address: Option<String>,
    pub destination: Option<Identity>,
}
impl Patch {
    pub fn settings(settings: Value) -> Self {
        Self {
            settings,
            ..Self::default()
        }
    }
    fn empty(&self) -> bool {
        self.address.is_none()
            && self.destination.is_none()
            && FIELDS.iter().all(|k| self.settings.get(k).is_none())
    }
}
/// Reserved app_configs keys; E0 initializes the fire marker and takeover journals.
pub const FIRED_AT_KEY: &str = "rust_rules_fired_at";
pub const IMPORT_KEY: &str = "rust_rule_import";
pub const JOB_CLEANUP_KEY: &str = "rust_rule_job_cleanup";

const FIELDS: &[&str] = &[
    "withdrawal_percentage",
    "threshold_type",
    "max_fee_percentage",
    "min_amount",
    "max_interval",
];
const DESTINATION_FIELDS: &[&str] = &["network", "address_tag", "address_name"];

#[derive(Clone, Debug)]
pub enum Command {
    Create {
        exchange_id: i64,
        asset_id: i64,
        patch: Patch,
    },
    Update(i64, Patch),
    /// A tile may submit settings with Start; the whole operation is atomic.
    Start(i64, Patch),
    Stop(i64),
    Delete(i64),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    NotFound,
    ValidationFailed,
    PermissionDenied,
    Conflict,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub status: Status,
    pub code: &'static str,
    pub reason: String,
}
fn refuse(status: Status, code: &'static str, reason: impl Into<String>) -> Refusal {
    Refusal {
        status,
        code,
        reason: reason.into(),
    }
}
fn missing() -> Refusal {
    refuse(Status::NotFound, "rule_not_found", "Rule not found.")
}
fn unconfirmed() -> Refusal {
    refuse(
        Status::Conflict,
        "withdrawal_unconfirmed",
        "The withdrawal status is unconfirmed. Resolve it before changing this rule.",
    )
}
fn not_listed() -> Refusal {
    refuse(
        Status::ValidationFailed,
        "address_not_listed",
        "That address is not on the exchange's withdrawal allowlist",
    )
}
#[derive(Debug)]
pub struct Outcome {
    pub id: i64,
    pub status: &'static str,
}

/// Name resolution for REST/MCP belongs to the same service as the web's ID-based create.
pub async fn create_named(
    app: &App,
    user: i64,
    exchange_name: &str,
    asset_symbol: &str,
    patch: Patch,
) -> Result<Result<Outcome, Refusal>, WebError> {
    if crate::ruby::blank(exchange_name)
        || crate::ruby::blank(asset_symbol)
        || patch.address.as_deref().is_none_or(crate::ruby::blank)
    {
        return Ok(Err(refuse(
            Status::ValidationFailed,
            "missing_required_parameter",
            "exchange_name, asset and address are required.",
        )));
    }
    let (name, symbol) = (exchange_name.to_owned(), asset_symbol.to_uppercase());
    let resolved=app.db(move|c| {
        let exchange:Option<(i64,String)>=c.query_row("SELECT id,type FROM exchanges WHERE available=1 AND LOWER(name)=LOWER(?1)",[name],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let Some((exchange,class))=exchange.filter(|(_,class)|!crate::web::bot::retired_type(class)) else {
            return Ok(Err(refuse(Status::NotFound,"exchange_not_found","Exchange not found.")));
        };
        if crate::venue::registry::row(&class).is_none_or(|v|v.withdrawals.is_none()) {
            return Ok(Err(refuse(Status::ValidationFailed,"withdrawal_unsupported","This exchange does not support withdrawals here.")));
        }
        let asset:Option<i64>=c.query_row("SELECT a.id FROM assets a JOIN tickers t ON (t.base_asset_id=a.id OR t.quote_asset_id=a.id) WHERE t.exchange_id=?1 AND t.available=1 AND t.trading_enabled=1 AND a.symbol=?2 ORDER BY CASE WHEN t.base_asset_id=a.id THEN 0 ELSE 1 END,a.id LIMIT 1",(exchange,symbol),|r|r.get(0)).optional()?;
        Ok(asset.map(|asset|(exchange,asset)).ok_or_else(||refuse(Status::NotFound,"asset_not_found","Asset is not listed on this exchange.")))
    }).await?;
    let (exchange_id, asset_id) = match resolved {
        Ok(ids) => ids,
        Err(e) => return Ok(Err(e)),
    };
    call(
        app,
        user,
        Command::Create {
            exchange_id,
            asset_id,
            patch,
        },
    )
    .await
}
#[derive(Debug)]
pub struct UnresolvedWithdrawal {
    pub state: String,
    pub reason: Option<String>,
    pub since: String,
    /// E0 supplies lookup candidates; W2 never performs recovery or sends.
    pub candidates: Vec<Value>,
}
#[derive(Debug)]
pub struct ListedRule {
    pub id: i64,
    pub rule_type: &'static str,
    pub status: &'static str,
    pub exchange: String,
    pub asset: String,
    pub address: String,
    pub address_tag: Value,
    pub network: Value,
    pub threshold_type: Value,
    pub withdrawal_percentage: Value,
    pub max_fee_percentage: Value,
    pub min_amount: Value,
    pub max_interval: Value,
    pub unresolved_withdrawal: Option<UnresolvedWithdrawal>,
}
fn status(n: i64) -> &'static str {
    match n {
        0 => "created",
        1 => "scheduled",
        2 => "stopped",
        3 => "deleted",
        4 => "executing",
        5 => "retrying",
        6 => "waiting",
        _ => "archived",
    }
}
pub fn mask(address: &str) -> String {
    let chars: Vec<_> = address.chars().collect();
    match chars.len() {
        0..=4 => "…".into(),
        5..=12 => chars[..2].iter().collect::<String>() + "…",
        n => {
            chars[..6].iter().collect::<String>() + "…" + &chars[n - 4..].iter().collect::<String>()
        }
    }
}
pub(super) fn visible_rules(c: &Connection, app: &App, user: i64) -> Result<Vec<Rule>, WebError> {
    let ids = c.prepare("SELECT id FROM rules WHERE user_id=?1 AND type='Rules::Withdrawal' AND status<>3 ORDER BY id")?
        .query_map([user], |r| r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
    let mut rules = Vec::new();
    for id in ids {
        if let Some(rule) = model::load(c, app, user, id)? {
            rules.push(rule);
        }
    }
    Ok(rules)
}
pub fn list(c: &Connection, app: &App, user: i64) -> Result<Vec<ListedRule>, WebError> {
    let mut rows = Vec::new();
    for r in visible_rules(c, app, user)? {
        let id = r.id;
        let unresolved_withdrawal = c.query_row(
            "SELECT state,reason,created_at FROM rust_withdrawal_intents WHERE owner=?1 AND state IN ('dispatched','unresolved')",
            [format!("rule:{id}")], |r| Ok(UnresolvedWithdrawal { state:r.get(0)?,reason:r.get(1)?,since:r.get(2)?,candidates:vec![] })).optional()?;
        rows.push(ListedRule {
            id,
            rule_type: "Rules::Withdrawal",
            status: status(r.status),
            exchange: r.exchange.name.clone(),
            asset: r.asset.symbol().into(),
            address: mask(&r.address),
            address_tag: r.settings["address_tag"].clone(),
            network: r.settings["network"].clone(),
            threshold_type: r.settings["threshold_type"].clone(),
            withdrawal_percentage: r.settings["withdrawal_percentage"].clone(),
            max_fee_percentage: r.settings["max_fee_percentage"].clone(),
            min_amount: r.settings["min_amount"].clone(),
            max_interval: r.settings["max_interval"].clone(),
            unresolved_withdrawal,
        });
    }
    Ok(rows)
}

fn bounds(rule: &Rule) -> Result<(), Refusal> {
    let threshold = rule.text("threshold_type");
    if !matches!(threshold.as_str(), "fee_percentage" | "min_amount") {
        return Err(refuse(
            Status::ValidationFailed,
            "invalid_threshold_type",
            "threshold_type must be one of: fee_percentage, min_amount.",
        ));
    }
    let required = if threshold == "min_amount" {
        "min_amount"
    } else {
        "max_fee_percentage"
    };
    if crate::ruby::blank(&rule.text(required)) {
        return Err(refuse(
            Status::ValidationFailed,
            "missing_required_parameter",
            format!("{required} is required for threshold_type {threshold}."),
        ));
    }
    for (field, cap) in [
        ("withdrawal_percentage", true),
        ("max_fee_percentage", true),
        ("min_amount", false),
    ] {
        let value = rule.text(field);
        if crate::ruby::blank(&value) && field != "withdrawal_percentage" {
            continue;
        }
        if !BigDec::parse(&value)
            .is_ok_and(|n| n.is_positive() && (!cap || n <= BigDec::from_i64(100)))
        {
            return Err(refuse(
                Status::ValidationFailed,
                "invalid_number",
                format!(
                    "{field} must be a number greater than 0{}.",
                    if cap { " and at most 100" } else { "" }
                ),
            ));
        }
    }
    // Rails treats max_interval as days. Accept a positive integer or no deadline.
    let interval = rule.text("max_interval");
    if !crate::ruby::blank(&interval) && !interval.parse::<i64>().is_ok_and(|n| n > 0) {
        return Err(refuse(
            Status::ValidationFailed,
            "invalid_number",
            "max_interval must be a positive number of days.",
        ));
    }
    Ok(())
}
fn has_intent(c: &Connection, id: i64) -> Result<bool, WebError> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM rust_withdrawal_intents WHERE owner=?1)",
        [format!("rule:{id}")],
        |r| r.get(0),
    )?)
}
fn guard(
    c: &Connection,
    app: &App,
    r: &Rule,
    cmd: &Command,
) -> Result<Result<(), Refusal>, WebError> {
    if !matches!(cmd, Command::Create { .. }) && r.status == 3 {
        return Ok(Err(missing()));
    }
    if !matches!(cmd, Command::Stop(_)) && has_intent(c, r.id)? {
        return Ok(Err(unconfirmed()));
    }
    let failure = match cmd {
        Command::Create { .. } if r.id != 0 && r.status != 3 => Some(refuse(
            Status::Conflict,
            "rule_exists",
            "A withdrawal rule for this asset and exchange already exists.",
        )),
        Command::Update(..) | Command::Delete(_) if !matches!(r.status, 0 | 2) => Some(refuse(
            Status::Conflict,
            "rule_active",
            if matches!(cmd, Command::Delete(_)) {
                "Stop the withdrawal rule before deleting it."
            } else {
                "Stop the withdrawal rule before updating it."
            },
        )),
        Command::Start(..) if !matches!(r.status, 0 | 2) => Some(refuse(
            Status::Conflict,
            "rule_already_active",
            "The withdrawal rule is already active.",
        )),
        Command::Stop(_) if !matches!(r.status, 1 | 4 | 5 | 6) => Some(refuse(
            Status::Conflict,
            "rule_not_active",
            "The withdrawal rule is not active.",
        )),
        _ => None,
    };
    if let Some(failure) = failure {
        return Ok(Err(failure));
    }
    if matches!(cmd, Command::Create { .. } | Command::Start(..)) {
        let (class, available): (String, bool) = c.query_row(
            "SELECT type,available FROM exchanges WHERE id=?1",
            [r.exchange.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if class != r.exchange.class {
            return Ok(Err(refuse(
                Status::Conflict,
                "exchange_changed",
                "The exchange changed. Reload the rule and try again.",
            )));
        }
        if matches!(cmd, Command::Create { .. }) && !available {
            return Ok(Err(refuse(
                Status::NotFound,
                "exchange_not_found",
                "Exchange not found.",
            )));
        }

        if crate::web::bot::retired_type(&r.exchange.class) {
            return Ok(Err(refuse(
                Status::ValidationFailed,
                "exchange_retired",
                "This exchange is retired.",
            )));
        }
        let capability = crate::venue::registry::row(&r.exchange.class);
        if capability.is_none_or(|v| v.withdrawals.is_none()) {
            return Ok(Err(refuse(
                Status::ValidationFailed,
                "withdrawal_unsupported",
                "This exchange does not support withdrawals here.",
            )));
        }
        let key =
            crate::engine::model::withdrawal_credentials(c, &app.cipher, r.user_id, r.exchange.id)?;
        if key.is_none() || !c.query_row("SELECT EXISTS(SELECT 1 FROM api_keys WHERE user_id=?1 AND exchange_id=?2 AND key_type=1 AND status=1)",(r.user_id,r.exchange.id),|r|r.get::<_,bool>(0))? {
            return Ok(Err(refuse(Status::PermissionDenied,"withdrawal_key_missing",format!("No valid withdrawal API key for {}. Add one in Settings.",r.exchange.name))));
        }
        if matches!(cmd, Command::Start(..)) && !capability.is_some_and(|v| v.withdraws) {
            return Ok(Err(refuse(
                Status::ValidationFailed,
                "withdrawal_disabled",
                "Withdrawals are not enabled for this exchange.",
            )));
        }
    }
    Ok(Ok(()))
}
fn apply_patch(r: &mut Rule, patch: &Patch, create: bool) -> Result<(), Refusal> {
    if create {
        r.settings = json!({"withdrawal_percentage":"100","threshold_type":"fee_percentage","max_fee_percentage":"0.5","min_amount":"0.1","max_interval":null});
    }
    for field in FIELDS.iter().chain(DESTINATION_FIELDS) {
        if let Some(value) = patch.settings.get(field) {
            let value = model::string(value);
            if *field == "max_interval" && crate::ruby::blank(&value) {
                r.settings[field.to_string()] = Value::Null;
            } else if !crate::ruby::blank(&value) {
                r.settings[field.to_string()] = json!(value);
            }
        }
    }
    if let Some(address) = &patch.address {
        if !create && *address != r.address && patch.settings.get("address_name").is_none() {
            r.settings["address_name"] = Value::Null;
        }
        r.address = address.clone();
    }
    if let Some(identity) = &patch.destination {
        let kind = crate::venue::withdrawal::Kind::of(&r.exchange.class).ok_or_else(not_listed)?;
        identity.apply(&mut r.address, &mut r.settings, kind);
    }
    if r.settings["withdrawal_percentage"].is_null() {
        r.settings["withdrawal_percentage"] = json!("100");
    }
    bounds(r)
}
fn consume(c: &Connection, id: i64, at: &str) -> rusqlite::Result<usize> {
    c.execute("UPDATE rust_rule_evaluations SET consumed_at=?1,outcome='skipped' WHERE rule_id=?2 AND consumed_at IS NULL",(at,id))
}
fn start_evaluations(c: &Connection, at: &str) -> rusqlite::Result<usize> {
    // D-RULE-EVALUATION-ID: one distinct fire per Start, fan-out matches EvaluateAllJob.
    c.execute("INSERT INTO rust_rule_evaluations(rule_id,source,created_at) SELECT id,?1,?2 FROM rules WHERE type='Rules::Withdrawal' AND status=1",
        (format!("start:{}",uuid::Uuid::new_v4()),at))
}
fn save(c: &Connection, app: &App, r: &Rule, at: &str) -> Result<(), WebError> {
    let old: (String, Option<String>, i64) = c.query_row(
        "SELECT settings,address,status FROM rules WHERE id=?1",
        [r.id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let settings = r.settings.to_string();
    let same_address = old
        .1
        .as_ref()
        .and_then(|v| app.cipher.decrypt(v).ok())
        .as_deref()
        == Some(&r.address);
    if old.0 == settings && old.2 == r.status && same_address {
        return Ok(());
    }
    let address = if same_address {
        old.1
    } else {
        Some(app.cipher.encrypt(&r.address))
    };
    c.execute("UPDATE rules SET status=?1,settings=?2,address=?3,updated_at=?4,settings_changed_at=CASE WHEN settings<>?2 THEN ?4 ELSE settings_changed_at END WHERE id=?5 AND user_id=?6",
        params![r.status,settings,address,at,r.id,r.user_id])?;
    Ok(())
}

/// Owns validation, live destination reads and the final immediate transaction.
/// Reads never authorize a changed rule or a replaced credential.
pub async fn call(
    app: &App,
    user: i64,
    cmd: Command,
) -> Result<Result<Outcome, Refusal>, WebError> {
    let (inner, command) = (app.clone(), cmd.clone());
    let loaded=app.db(move|c| {
        let tx=c.unchecked_transaction()?;
        let r=match &command {
            Command::Create{exchange_id,asset_id,..} => {
                let Some(exchange)=crate::web::wizard::keys::venue(&tx,Some(*exchange_id))? else {
                    return Ok(Err(refuse(Status::NotFound,"exchange_not_found","Exchange not found.")));
                };
                if *asset_id==0 { return Ok(Err(refuse(Status::ValidationFailed,"missing_required_parameter","asset and address are required."))) }
                let Some(asset)=crate::web::bot::Asset::find(&tx,*asset_id)? else {
                    return Ok(Err(refuse(Status::NotFound,"asset_not_found","Asset not found.")));
                };
                let listed:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM tickers WHERE exchange_id=?1 AND available=1 AND trading_enabled=1 AND (base_asset_id=?2 OR quote_asset_id=?2))",(*exchange_id,*asset_id),|r|r.get(0))?;
                if !listed { return Ok(Err(refuse(Status::NotFound,"asset_not_found","Asset is not listed on this exchange."))) }
                let id:Option<i64>=tx.query_row("SELECT id FROM rules WHERE user_id=?1 AND type='Rules::Withdrawal' AND exchange_id=?2 AND asset_id=?3",(user,exchange_id,asset_id),|r|r.get(0)).optional()?;
                if let Some(id)=id { model::load(&tx,&inner,user,id)?.ok_or_else(||WebError::Config("rule disappeared".into()))? }
                else { Rule {id:0,user_id:user,status:0,exchange,asset,address:String::new(),settings:json!({}),fee:None,chains:vec![],balance:None,unconfirmed:None} }
            },
            Command::Update(id,_)|Command::Start(id,_)|Command::Stop(id)|Command::Delete(id) => {
                let Some(rule)=model::load(&tx,&inner,user,*id)? else { return Ok(Err(missing())) };
                rule
            }
        };
        if let Err(e)=guard(&tx,&inner,&r,&command)? { return Ok(Err(e)) }
        let revision=if r.id==0 {None} else {Some(tx.query_row("SELECT updated_at FROM rules WHERE id=?1",[r.id],|r|r.get::<_,String>(0))?)};
        Ok(Ok((r,revision)))
    }).await?;
    let (mut r, revision) = match loaded {
        Ok(r) => r,
        Err(e) => return Ok(Err(e)),
    };
    let original = r.clone();
    let patch = match &cmd {
        Command::Create { patch, .. } | Command::Update(_, patch) | Command::Start(_, patch) => {
            Some(patch)
        }
        _ => None,
    };
    let mut version = None;
    if let Some(patch) = patch {
        if matches!(cmd, Command::Update(..)) && patch.empty() {
            return Ok(Err(refuse(
                Status::ValidationFailed,
                "no_updates_provided",
                "No settings provided to update.",
            )));
        }
        if let Err(e) = apply_patch(&mut r, patch, matches!(cmd, Command::Create { .. })) {
            return Ok(Err(e));
        }
        if r.address.is_empty() {
            return Ok(Err(refuse(
                Status::ValidationFailed,
                "missing_required_parameter",
                "address is required.",
            )));
        }
        let Some(list) = reads::listing(app, &r, true).await? else {
            return Ok(Err(not_listed()));
        };
        if patch
            .destination
            .as_ref()
            .is_some_and(|i| !i.listed(&r, &list))
        {
            return Ok(Err(not_listed()));
        }
        let Ok(destination) = reads::select(&r, &list) else {
            return Ok(Err(not_listed()));
        };
        reads::store(&mut r.settings, &destination);
        version = Some(list.version);
    }
    let inner = app.clone();
    let at = app.now().format("%Y-%m-%d %H:%M:%S%.6f").to_string();
    app.db(move|c| {
        let tx=crate::engine::model::immediate(c)?;
        if let Some(version)=version {
            let eligible:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM api_keys WHERE id=?1 AND user_id=?2 AND exchange_id=?3 AND key_type=1 AND status=1)",(version.id,user,r.exchange.id),|r|r.get(0))?;
            if !eligible || !crate::engine::model::credential_is_current(&tx,&version)? {return Ok(Err(refuse(Status::Conflict,"credential_changed","The withdrawal key changed. Try again.")))}
        }
        if original.id!=0 {
            let Some(current)=model::load(&tx,&inner,user,original.id)? else {return Ok(Err(missing()))};
            if let Err(e)=guard(&tx,&inner,&current,&cmd)? {return Ok(Err(e))}
            let current_revision:String=tx.query_row("SELECT updated_at FROM rules WHERE id=?1",[r.id],|r|r.get(0))?;
            if Some(current_revision)!=revision || current.exchange.id!=original.exchange.id || current.asset.id!=original.asset.id || current.status!=original.status || current.settings!=original.settings || current.address!=original.address || current.exchange.class!=original.exchange.class {
                return Ok(Err(refuse(Status::Conflict,"rule_changed","The rule changed. Reload it and try again.")));
            }
        } else {
            if tx.query_row("SELECT EXISTS(SELECT 1 FROM rules WHERE user_id=?1 AND type='Rules::Withdrawal' AND exchange_id=?2 AND asset_id=?3)",(user,r.exchange.id,r.asset.id),|r|r.get::<_,bool>(0))? {
                return Ok(Err(refuse(Status::Conflict,"rule_exists","A withdrawal rule for this asset and exchange already exists.")));
            }
            if let Err(e)=guard(&tx,&inner,&r,&cmd)? {return Ok(Err(e))}
        }
        r.status=match cmd {Command::Create{..}=>0,Command::Start(..)=>1,Command::Stop(_)=>2,Command::Delete(_)=>3,Command::Update(..)=>r.status};
        if r.id==0 {
            tx.execute("INSERT INTO rules(user_id,type,status,exchange_id,asset_id,address,settings,created_at,updated_at,settings_changed_at) VALUES (?1,'Rules::Withdrawal',0,?2,?3,?4,?5,?6,?6,?6)",
                params![user,r.exchange.id,r.asset.id,inner.cipher.encrypt(&r.address),r.settings.to_string(),at])?;
            r.id=tx.last_insert_rowid();
        } else { save(&tx,&inner,&r,&at)?; }
        match cmd {
            Command::Start(..)=>{start_evaluations(&tx,&at)?;},
            Command::Stop(_)|Command::Delete(_)|Command::Create{..}=>{consume(&tx,r.id,&at)?;},
            Command::Update(..)=>{},
        }
        tx.commit()?;
        Ok(Ok(Outcome { id:r.id,status:status(r.status) }))
    }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn starts_fan_out_once_per_start_in_the_status_transaction() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE rules(id INTEGER PRIMARY KEY,type TEXT,status INTEGER); INSERT INTO rules VALUES (1,'Rules::Withdrawal',0),(2,'Rules::Withdrawal',1),(3,'Rules::Withdrawal',2),(4,'Other',1);").unwrap();
        c.execute_batch(include_str!(
            "../../../migrations/20261026000001_withdrawal_rules.sql"
        ))
        .unwrap();
        for _ in 0..2 {
            let tx = crate::engine::model::immediate(&c).unwrap();
            tx.execute("UPDATE rules SET status=1 WHERE id=1", [])
                .unwrap();
            assert_eq!(start_evaluations(&tx, "2026-10-26").unwrap(), 2);
            tx.commit().unwrap();
        }
        let mut stmt=c.prepare("SELECT source,count(*),count(DISTINCT rule_id) FROM rust_rule_evaluations GROUP BY source").unwrap();
        let groups = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(groups.len(), 2);
        for (source, count, rules) in groups {
            uuid::Uuid::parse_str(source.strip_prefix("start:").unwrap()).unwrap();
            assert_eq!((count, rules), (2, 2));
        }
        {
            let tx = crate::engine::model::immediate(&c).unwrap();
            tx.execute("UPDATE rules SET status=1 WHERE id=3", [])
                .unwrap();
            assert_eq!(start_evaluations(&tx, "2026-10-27").unwrap(), 3);
            // A failed commit must roll back both the status and its evaluations.
        }
        assert_eq!(
            c.query_row("SELECT status FROM rules WHERE id=3", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM rust_rule_evaluations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            4
        );
    }
}
