//! Durable rule evaluations and Rails handover evidence. D-RULE-AMBIGUITY-ALLOWLIST:
//! only a listed rejection proves absence; a later success never discharges an earlier attempt.
use super::{model, EngineError};
use crate::{app_config, codec::format_time};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub const IMPORT: &str = "rust_rule_import";
pub const CLEANUP: &str = "rust_rule_job_cleanup";
pub const FIRED: &str = "rust_rules_fired_at";

/// Exact messages and anchored templates. %s and {0} each stand for one nonempty value.
/// Sources: Binance wallet error-code (https://developers.binance.com/docs/wallet/error-code),
/// Binance spot errors (https://developers.binance.com/docs/binance-spot-api-docs/errors),
/// Kraken spot errors (https://docs.kraken.com/api/docs/guides/spot-errors/),
/// Gemini errors (https://docs.gemini.com/error-codes), MEXC spot v3 error-code
/// (https://mexcdevelop.github.io/apidocs/spot_v3_en/#error-code). Rails aliases are in
/// app/models/exchanges/{binance:3-9,kraken:37,mexc:9-10}. Unknown messages stay ambiguous.
const DEFINITIVE: &[(&str, &[&str])] = &[
 ("Binance", &[
  "Support main account only.", "Address validation is not passed.", "Address tag validation is not passed.",
  "White list is not opened.", "2FA is not opened.", "Withdraw is not allowed within 2 min login.", "Withdraw is limited.",
  "Within 24 hours after password modification, withdrawal is prohibited.",
  "Within 24 hours after the release of 2FA, withdrawal is prohibited.", "We don't have this asset.",
  "Current asset is not open for withdrawal.", "Asset withdrawal must be an %s multiple of %s.",
  "Not less than the minimum pick-up quantity %s.", "Within 24 hours, the withdrawal exceeds the maximum amount.",
  "You don't have this asset.", "The number of hold asset is less than zero.", "You have insufficient balance.",
  "The amount of withdrawal must be greater than the Commission.", "Illegal address.", "The address is suspected of fake.",
  "This address is not on the whitelist. Please join and try again.", "The new address needs to be withdrawn in {0} hours.",
  "As your deposit has not reached the required block confirmations, we have temporarily locked {0} asset",
  "This network does not support memo/tag. Please remove the `addressTag` field and try again.",
  "You are not authorized to execute this request.", "Invalid API-key, IP, or permissions for action.",
  "API-key format invalid.", "Invalid Api-Key ID.", "Account has insufficient balance for requested action.",
  "Timestamp for this request is outside of the recvWindow.", "Timestamp for this request is outside of the recvWindow",
  "Timestamp for this request was %s ahead of the server's time.",
  "Illegal characters found in a parameter.", "Illegal characters found in a parameter. %s", "Illegal characters found in parameter `%s`; legal range is `%s`.", "Too many parameters sent for this endpoint.",
  "Too many parameters; expected `%s` and received `%s`.", "Duplicate values for a parameter detected.",
  "A mandatory parameter was not sent, was empty/null, or malformed.", "Mandatory parameter `%s` was not sent, was empty/null, or malformed.",
  "An unknown parameter was sent.", "Not all sent parameters were read.", "Not all sent parameters were read; read `%s` parameter(s) but was sent `%s`.",
  "A parameter was empty.", "Parameter `%s` was empty.", "A parameter was sent when not required.", "Parameter `%s` sent when not required.",
  "Parameter `%s` has too much precision.", "Precision is over the maximum defined for this asset.", "Combination of optional parameters invalid.",
  "Invalid data sent for a parameter.", "Data sent for parameter `%s` is not valid.", "Param `%s` or `%s` must be sent, but both were empty/null!"
 ]),
 ("Kraken", &[
  "EFunding:Max fee exceeded", "EBM:limit exceeded:CAL", "EGeneral:Invalid arguments", "EGeneral:Invalid arguments:%s",
  "EGeneral:Permission denied", "EAPI:Invalid key", "EAPI:Invalid signature", "EAPI:Invalid nonce",
  "EGeneral:Temporary lockout", "EAPI:Insufficient funds", "EOrder:Insufficient funds"
 ]),
 ("Mexc", &[
  "amount cannot be null", "amount decimal places is too long", "amount is error", "amount cannot be zero or negative", "recvWindow must less than 60000", "Illegal characters found in parameter",
  "Insufficient balance", "This withdrawal address is not on the commonly used address list or has been invalidated",
  "Due to risk control, withdrawal is unavailable, please try again later", "currency cannot be null", "currency does not exist",
  "remark length is too long", "param is error", "param cannot be null", "api key required", "No authority",
  "Signature verification failed", "invalid access key", "API-key format invalid", "Signature for this request is not valid",
  "Timestamp for this request is outside of the recvWindow", "IP non white list", "No permission to access the endpoint",
  "Your input param is invalid", "Parameter error", "Api key info invalid", "Invalid Api-Key ID."
 ]),
 ("Gemini", &[
  "InsufficientFunds", "InvalidSignature", "InvalidApiKey", "InvalidNonce", "MissingRole", "MissingApikeyHeader",
  "MissingPayloadHeader", "MissingSignatureHeader", "MissingPayloadKey", "MissingNonce", "InvalidJson", "EndpointMismatch",
  "EndpointNotFound", "InvalidQuantity", "AccountClosed", "RemoteAddressForbidden"
 ]),
];
fn template_matches(pattern: &str, text: &str) -> bool {
    let pattern = regex::escape(pattern)
        .replace("%s", ".+")
        .replace(r"\{0\}", ".+");
    regex::Regex::new(&format!("\\A{pattern}\\z"))
        .expect("escaped allowlist")
        .is_match(text)
}
pub fn definitive(exchange: &str, error: &str) -> bool {
    // Rails pre-transmission errors (withdraw methods); these branches are not generated by Rust.
    if template_matches("Unknown symbol for asset %s", error)
        || template_matches("No coin data found for %s on MEXC", error)
    {
        return true;
    }
    let exchange = exchange.strip_prefix("Exchanges::").unwrap_or(exchange);
    let exchange = if exchange == "BinanceUs" {
        "Binance"
    } else {
        exchange
    };
    let Some((_, list)) = DEFINITIVE.iter().find(|(v, _)| *v == exchange) else {
        return false;
    };
    if exchange == "Gemini" {
        return serde_json::from_str::<Value>(error).ok().is_some_and(|v| {
            v["result"] == "error" && v["reason"].as_str().is_some_and(|r| list.contains(&r))
        });
    }
    list.iter().any(|pattern| template_matches(pattern, error))
}
pub fn ambiguous(exchange: &str, message: &str) -> bool {
    if message.starts_with("Unexpected error:") {
        return true;
    }
    if let Some(error) = message.strip_prefix("Withdrawal failed: ") {
        return !definitive(exchange, error);
    }
    message.starts_with("Withdrawal failed")
}
/// V1 scrubbing, tightened for durable messages: no URL (including path), no query or signature, <=200 chars.
pub fn safe_message(message: &str) -> String {
    if message.to_ascii_lowercase().contains("<html")
        || message.to_ascii_lowercase().contains("<!doctype")
    {
        return "the venue answered with an HTML page".into();
    }
    let urls = regex::Regex::new(r#"https?://[^\s\"'<>]+"#).expect("static URL regex");
    let without_urls = urls.replace_all(message, "[venue request]");
    let secrets = regex::Regex::new(r"[A-Za-z][A-Za-z0-9_-]*=[^\s]*").expect("static query regex");
    let clean = secrets.replace_all(&without_urls, "[redacted]");
    crate::venue::withdrawal::log_safe(&clean)
        .chars()
        .take(200)
        .collect()
}
pub fn log(
    c: &Connection,
    rule: i64,
    client: Option<&str>,
    status: i64,
    message: &str,
    details: &Value,
    now: DateTime<Utc>,
) -> Result<i64, EngineError> {
    if c.is_autocommit() {
        return Err(EngineError::Data(
            "rule outcome requires a transaction".into(),
        ));
    }
    c.execute(
        "INSERT INTO rule_logs(rule_id,status,message,details,created_at) VALUES(?1,?2,?3,?4,?5)",
        params![
            rule,
            status,
            safe_message(message),
            details.to_string(),
            format_time(now)
        ],
    )?;
    let id = c.last_insert_rowid();
    c.execute("INSERT INTO rust_rule_log_origins(rule_log_id,owner,client_id,created_at) VALUES(?1,?2,?3,?4)",params![id,format!("rule:{rule}"),client,format_time(now)])?;
    Ok(id)
}
pub fn seed_rails_ids(c: &Connection) -> Result<(), EngineError> {
    c.execute("INSERT OR IGNORE INTO rust_withdrawal_ids(exchange_id,venue_id,owner,settled_at,source) SELECT r.exchange_id,CAST(json_extract(l.details,'$.withdrawal_id') AS TEXT),'rule:'||r.id,l.created_at,'rails' FROM rule_logs l JOIN rules r ON r.id=l.rule_id WHERE l.status=1 AND json_valid(l.details) AND json_extract(l.details,'$.withdrawal_id') IS NOT NULL",[])?;
    Ok(())
}
pub fn table_exists(c: &Connection, name: &str) -> rusqlite::Result<bool> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |r| r.get(0),
    )
}
pub fn open_attempts(c: &Connection, rule: Option<i64>) -> Result<Vec<(i64, i64)>, EngineError> {
    let acks = if table_exists(c, "rust_rails_attempt_acks")? {
        "AND NOT EXISTS(SELECT 1 FROM rust_rails_attempt_acks a WHERE a.rule_log_id=l.id)"
    } else {
        ""
    };
    let origins = if table_exists(c, "rust_rule_log_origins")? {
        "AND NOT EXISTS(SELECT 1 FROM rust_rule_log_origins o WHERE o.rule_log_id=l.id)"
    } else {
        ""
    };
    let mut s=c.prepare(&format!("SELECT l.id,l.rule_id,e.type,l.message FROM rule_logs l JOIN rules r ON r.id=l.rule_id JOIN exchanges e ON e.id=r.exchange_id WHERE (?1 IS NULL OR l.rule_id=?1) {acks} {origins} ORDER BY l.id"))?;
    let rows = s
        .query_map([rule], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .filter(|(_, _, e, m)| m.as_deref().is_some_and(|m| ambiguous(e, m)))
        .map(|(id, r, _, _)| (id, r))
        .collect())
}
#[derive(Clone, Debug)]
pub struct Job {
    pub id: String,
    pub rule: Option<i64>,
    pub all: bool,
    pub claimed: bool,
    pub acknowledged: bool,
}
pub fn jobs(c: &Connection, q: &Connection) -> Result<Vec<Job>, EngineError> {
    let mut s=q.prepare("SELECT j.active_job_id,j.class_name,j.arguments,EXISTS(SELECT 1 FROM solid_queue_claimed_executions e WHERE e.job_id=j.id) OR EXISTS(SELECT 1 FROM solid_queue_failed_executions e WHERE e.job_id=j.id) FROM solid_queue_jobs j WHERE j.class_name IN ('Rule::EvaluateAllJob','Rule::ExecuteJob') AND j.finished_at IS NULL AND (EXISTS(SELECT 1 FROM solid_queue_claimed_executions e WHERE e.job_id=j.id) OR EXISTS(SELECT 1 FROM solid_queue_failed_executions e WHERE e.job_id=j.id) OR EXISTS(SELECT 1 FROM solid_queue_ready_executions e WHERE e.job_id=j.id) OR EXISTS(SELECT 1 FROM solid_queue_scheduled_executions e WHERE e.job_id=j.id) OR (j.class_name='Rule::ExecuteJob' AND EXISTS(SELECT 1 FROM solid_queue_blocked_executions e WHERE e.job_id=j.id))) ORDER BY j.id")?;
    let rows = s
        .query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, bool>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut jobs = vec![];
    for (id, class, args, claimed) in rows {
        let id =
            id.ok_or_else(|| EngineError::Ineligible(vec![format!("{class}: no active job id")]))?;
        let all = class == "Rule::EvaluateAllJob";
        let rule = if all {
            None
        } else {
            let a: Value = serde_json::from_str(args.as_deref().unwrap_or("")).map_err(|_| {
                EngineError::Ineligible(vec![format!("{class} {id}: unreadable arguments")])
            })?;
            let arg = &a["arguments"][0];
            let gid = arg["_aj_globalid"]
                .as_str()
                .or_else(|| arg.as_str())
                .unwrap_or("");
            Some(
                gid.strip_prefix("gid://deltabadger/Rules::Withdrawal/")
                    .and_then(|s| s.parse::<i64>().ok())
                    .ok_or_else(|| {
                        EngineError::Ineligible(vec![format!("{class} {id}: unreadable rule")])
                    })?,
            )
        };
        let acknowledged = table_exists(c, "rust_rails_job_acks")?
            && c.query_row(
                "SELECT EXISTS(SELECT 1 FROM rust_rails_job_acks WHERE active_job_id=?1)",
                [&id],
                |r| r.get::<_, bool>(0),
            )?;
        jobs.push(Job {
            id,
            rule,
            all,
            claimed,
            acknowledged,
        });
    }
    Ok(jobs)
}
pub fn takeover_problems(c: &Connection, q: &Connection) -> Result<Vec<String>, EngineError> {
    let mut problems = open_attempts(c, None)?
        .into_iter()
        .map(|(id, rule)| {
            format!(
                "rule {rule}: Rails attempt {id} is unconfirmed; resolve-withdrawal --rails {id}"
            )
        })
        .collect::<Vec<_>>();
    for job in jobs(c, q)? {
        if job.claimed && !job.acknowledged {
            problems.push(format!(
                "{} {}{} was claimed or failed; resolve-withdrawal --rails-job {}",
                if job.all {
                    "Rule::EvaluateAllJob"
                } else {
                    "Rule::ExecuteJob"
                },
                job.id,
                job.rule.map_or(String::new(), |r| format!(" for rule {r}")),
                job.id
            ));
        }
    }
    Ok(problems)
}
fn journal(c: &Connection, key: &str) -> Result<Vec<String>, EngineError> {
    let Some(text) = app_config::get_plain(c, key).map_err(EngineError::Data)? else {
        return Ok(vec![]);
    };
    serde_json::from_str(&text).map_err(|_| EngineError::Data(format!("{key}: unreadable journal")))
}
pub fn append_journal(
    c: &Connection,
    key: &str,
    id: &str,
    now: DateTime<Utc>,
) -> Result<(), EngineError> {
    let mut set = journal(c, key)?;
    if !set.iter().any(|v| v == id) {
        set.push(id.into());
    }
    write_journal(c, key, &set, now)
}
fn write_journal(
    c: &Connection,
    key: &str,
    set: &[String],
    now: DateTime<Utc>,
) -> Result<(), EngineError> {
    if set.is_empty() {
        app_config::delete(c, key)
    } else {
        app_config::set_plain(c, key, &json!(set).to_string(), now)
    }
    .map_err(EngineError::Data)
}
pub fn journals_pending(c: &Connection) -> Result<bool, EngineError> {
    Ok(!journal(c, IMPORT)?.is_empty() || !journal(c, CLEANUP)?.is_empty())
}
/// In the claim transaction, before any queue cleanup. An unclaimed fanout has no children yet.
pub fn import(c: &Connection, jobs: &[Job], now: DateTime<Utc>) -> Result<usize, EngineError> {
    let mut added = 0;
    for job in jobs {
        if job.acknowledged {
            append_journal(c, CLEANUP, &job.id, now)?;
            continue;
        }
        if job.claimed {
            return Err(EngineError::Ineligible(vec![format!(
                "claimed rule job {}",
                job.id
            )]));
        }
        if journal(c, IMPORT)?.contains(&job.id) {
            continue;
        }
        let source = format!("rails:{}", job.id);
        added += if job.all {
            c.execute("INSERT OR IGNORE INTO rust_rule_evaluations(rule_id,source,created_at) SELECT id,?1,?2 FROM rules WHERE type='Rules::Withdrawal' AND status=1",params![source,format_time(now)])?
        } else {
            c.execute("INSERT OR IGNORE INTO rust_rule_evaluations(rule_id,source,created_at) SELECT id,?1,?2 FROM rules WHERE id=?3 AND type='Rules::Withdrawal'",params![source,format_time(now),job.rule])?
        };
        append_journal(c, IMPORT, &job.id, now)?;
    }
    seed_rails_ids(c)?;
    Ok(added)
}
pub fn cleanup_queue(c: &Connection, q: &Connection) -> Result<usize, EngineError> {
    let mut n = 0;
    for key in [IMPORT, CLEANUP] {
        for id in journal(c, key)? {
            n+=q.execute("DELETE FROM solid_queue_jobs WHERE active_job_id=?1 AND class_name IN ('Rule::EvaluateAllJob','Rule::ExecuteJob')",[id])?;
        }
    }
    Ok(n)
}
pub fn prune_journals(
    c: &Connection,
    q: &Connection,
    now: DateTime<Utc>,
) -> Result<(), EngineError> {
    let tx = model::immediate(c)?;
    for key in [IMPORT, CLEANUP] {
        let mut left = vec![];
        for id in journal(&tx, key)? {
            if q.query_row(
                "SELECT EXISTS(SELECT 1 FROM solid_queue_jobs WHERE active_job_id=?1)",
                [&id],
                |r| r.get::<_, bool>(0),
            )? {
                left.push(id);
            }
        }
        write_journal(&tx, key, &left, now)?;
    }
    tx.commit()?;
    Ok(())
}
pub fn finish_cleanup(
    c: &Connection,
    q: &Connection,
    now: DateTime<Utc>,
) -> Result<usize, EngineError> {
    let n = cleanup_queue(c, q)?;
    prune_journals(c, q, now)?;
    Ok(n)
}
pub fn start_evaluations(c: &Connection, now: DateTime<Utc>) -> Result<usize, EngineError> {
    Ok(c.execute("INSERT INTO rust_rule_evaluations(rule_id,source,created_at) SELECT id,?1,?2 FROM rules WHERE type='Rules::Withdrawal' AND status=1",params![format!("start:{}",uuid::Uuid::new_v4()),format_time(now)])?)
}
pub fn fire(c: &Connection, now: DateTime<Utc>) -> Result<usize, EngineError> {
    let at = super::run::RULES_SCHEDULE.last_fire(now);
    let tx = model::immediate(c)?;
    let last = app_config::get_plain(&tx, FIRED).map_err(EngineError::Data)?;
    let mut n = 0;
    if let Some(last) = last {
        let last = crate::codec::parse_time(&last).map_err(EngineError::Arithmetic)?;
        if at <= last {
            return Ok(0);
        }
        n=tx.execute("INSERT OR IGNORE INTO rust_rule_evaluations(rule_id,source,created_at) SELECT id,?1,?2 FROM rules WHERE type='Rules::Withdrawal' AND status=1",params![format!("fire:{}",at.to_rfc3339()),format_time(now)])?;
    }
    app_config::set_plain(&tx, FIRED, &at.to_rfc3339(), now).map_err(EngineError::Data)?;
    tx.commit()?;
    Ok(n)
}
pub fn handback_problems(c: &Connection) -> Result<Vec<String>, EngineError> {
    if !table_exists(c, "rust_rule_evaluations")? {
        return Ok(vec![]);
    }
    let mut s=c.prepare("SELECT owner FROM rust_withdrawal_intents UNION SELECT 'rule:'||rule_id FROM rust_rule_evaluations WHERE consumed_at IS NULL")?;
    let mut problems = s
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|owner| format!("{owner}: withdrawal work is unconsumed or unconfirmed"))
        .collect::<Vec<_>>();
    if journals_pending(c)? {
        problems.push("withdrawal job cleanup is pending".into());
    }
    Ok(problems)
}
#[derive(Clone, Debug)]
pub struct Status {
    pub state: String,
    pub reason: Option<String>,
    pub since: String,
    pub candidates: Vec<Value>,
}
pub fn status(c: &Connection, rule: i64) -> Result<Option<Status>, EngineError> {
    let owner = format!("rule:{rule}");
    let mut status=c.query_row("SELECT state,reason,created_at FROM rust_withdrawal_intents WHERE owner=?1 AND state IN ('dispatched','unresolved')",[&owner],|r|Ok(Status{state:r.get(0)?,reason:r.get(1)?,since:r.get(2)?,candidates:vec![]})).optional()?;
    if let Some(s) = &mut status {
        let raw:Option<String>=c.query_row("SELECT c.candidates FROM rust_withdrawal_candidates c JOIN rust_withdrawal_intents i ON i.owner=c.owner AND i.client_id=c.client_id WHERE c.owner=?1",[owner],|r|r.get(0)).optional()?;
        s.candidates = raw
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default();
    }
    Ok(status)
}
