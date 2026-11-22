//! Durable rule evaluations and 2.x handover evidence. D-RULE-AMBIGUITY-ALLOWLIST:
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
/// (https://mexcdevelop.github.io/apidocs/spot_v3_en/#error-code). 2.x aliases are in
/// app/models/exchanges/{binance:3-9,kraken:37,mexc:9-10}. Unknown messages stay ambiguous.
/// The numeric lists gate E1 wire replies; 2.x rows lost their codes and still use messages only.
const DEFINITIVE: &[(&str, &[i64], &[&str])] = &[
 ("Binance", &[
  -4006, -4007, -4008, -4012, -4013, -4014, -4015, -4016, -4017, -4018, -4019, -4021, -4022, -4023, -4024, -4025,
  -4026, -4028, -4033, -4034, -4035, -4036, -4060, -4106, -1002, -1100, -1101, -1102, -1103,
  -1104, -1105, -1106, -1111, -1128, -1130, -2015, -1021
 ], &[
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
 ("Kraken", &[], &[
  "EFunding:Max fee exceeded", "EBM:limit exceeded:CAL", "EGeneral:Invalid arguments", "EGeneral:Invalid arguments:%s",
  "EGeneral:Permission denied", "EAPI:Invalid key", "EAPI:Invalid signature", "EAPI:Invalid nonce",
  "EGeneral:Temporary lockout", "EAPI:Insufficient funds", "EOrder:Insufficient funds"
 ]),
 ("Mexc", &[
  10101, 10212, 10265, 10095, 10096, 10097, 10102, 10222, 10232, 10268, 33333, 44444,
  400, 401, 602, 10072, 700001, 700002, 700003, 700005, 700006, 700007, 700008, 730002
 ], &[
  "amount cannot be null", "amount decimal places is too long", "amount is error", "amount cannot be zero or negative", "recvWindow must less than 60000", "Illegal characters found in parameter",
  "Insufficient balance", "This withdrawal address is not on the commonly used address list or has been invalidated",
  "Due to risk control, withdrawal is unavailable, please try again later", "currency cannot be null", "currency does not exist",
  "remark length is too long", "param is error", "param cannot be null", "api key required", "No authority",
  "Signature verification failed", "invalid access key", "API-key format invalid", "Signature for this request is not valid",
  "Timestamp for this request is outside of the recvWindow", "IP non white list", "No permission to access the endpoint",
  "Your input param is invalid", "Parameter error", "Api key info invalid", "Invalid Api-Key ID."
 ]),
 ("Gemini", &[], &[
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
    // 2.x pre-transmission errors (withdraw methods); these branches are not generated by Rust.
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
    let Some((_, _, list)) = DEFINITIVE.iter().find(|(v, _, _)| *v == exchange) else {
        return false;
    };
    // Rust carries the complete Kraken reply through settlement. 2.x evidence remains
    // the original single error string below. Every error in a reply must prove rejection.
    if exchange == "Kraken" {
        if let Ok(body) = serde_json::from_str::<Value>(error) {
            return body["result"].is_null() && body["error"].as_array().is_some_and(|errors| {
                !errors.is_empty() && errors.iter().all(|e| e.as_str().is_some_and(|e| {
                    !e.starts_with("EGeneral:") && !e.starts_with("EService:")
                        && list.iter().any(|pattern| template_matches(pattern, e))
                }))
            });
        }
    }
    if exchange == "Gemini" {
        return serde_json::from_str::<Value>(error).ok().is_some_and(|v| {
            v["result"] == "error" && v["reason"].as_str().is_some_and(|r| list.contains(&r))
        });
    }
    list.iter().any(|pattern| template_matches(pattern, error))
}
/// E1 wire replies retain their numeric code. Require a listed code AND message; 2.x-only
/// pre-send texts cannot turn an unknown venue code into proof of rejection.
pub(crate) fn definitive_code(exchange: &str, code: i64, message: &str) -> bool {
    DEFINITIVE.iter().any(|(venue, codes, messages)| {
        *venue == exchange && codes.contains(&code)
            && messages.iter().any(|pattern| template_matches(pattern, message))
    })
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
pub fn seed_v2_ids(c: &Connection, now: DateTime<Utc>) -> Result<(), EngineError> {
    // Each (user, exchange) of 2.x's successes has its number 1 (eager rows; 2.x may have added one since the migration).
    let mut s = c.prepare("SELECT DISTINCT r.user_id,r.exchange_id FROM rule_logs l JOIN rules r ON r.id=l.rule_id WHERE l.status=1 AND r.exchange_id IS NOT NULL")?;
    let pairs = s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    drop(s);
    for (user, exchange) in pairs { crate::identity::touch(c, user, exchange, now)?; }
    // 2.x's successes: of 2.x's rules, each its (user, exchange)'s number 1 (#710; a second account's rule has no exchange there).
    c.execute("INSERT OR IGNORE INTO rust_withdrawal_ids(account_id,exchange_id,venue_id,owner,settled_at,source) SELECT a.id,r.exchange_id,CAST(json_extract(l.details,'$.withdrawal_id') AS TEXT),'rule:'||r.id,l.created_at,'v2' FROM rule_logs l JOIN rules r ON r.id=l.rule_id JOIN exchange_accounts a ON a.user_id=r.user_id AND a.exchange_id=r.exchange_id AND a.number=1 WHERE l.status=1 AND json_valid(l.details) AND json_extract(l.details,'$.withdrawal_id') IS NOT NULL",[])?;
    Ok(())
}

// ---- #710: withdrawal rules live in exchange_account_rules (migration 20261119000002), each with its account. 2.x's
// `rules` keeps a row for every id (rule_logs' foreign key, and the id itself comes from it) and, for a number-1 rule,
// the same row: every write to the store mirrors it in its own transaction, so 2.x, an export and a handback read what
// Rust runs. A rule of an account 2+ has only its id row there (deleted to 2.x, no exchange): 2.x could not hold it.

const COLUMNS: &str = "id,user_id,type,status,exchange_id,asset_id,address,settings,settings_changed_at,created_at,updated_at";
const UPDATED: &str = "user_id=excluded.user_id,type=excluded.type,status=excluded.status,exchange_id=excluded.exchange_id,asset_id=excluded.asset_id,\
                       address=excluded.address,settings=excluded.settings,settings_changed_at=excluded.settings_changed_at,created_at=excluded.created_at,updated_at=excluded.updated_at";

/// 2.x's `rules` row of the store's number-1 rule `id` (every one with None) made equal to it.
pub fn mirror(c: &Connection, id: Option<i64>) -> rusqlite::Result<usize> {
    c.execute(&format!("INSERT INTO rules({COLUMNS}) SELECT r.id,r.user_id,r.type,r.status,r.exchange_id,r.asset_id,r.address,r.settings,r.settings_changed_at,r.created_at,r.updated_at \
                        FROM exchange_account_rules r JOIN exchange_accounts a ON a.id=r.account_id AND a.number=1 WHERE ?1 IS NULL OR r.id=?1 ON CONFLICT(id) DO UPDATE SET {UPDATED}"), [id])
}

/// A new withdrawal rule of `account` (with its row): its id from 2.x's `rules`, then the store's row, then the mirror.
pub fn create(c: &Connection, account: &crate::identity::Account, asset_id: i64, address: &str, settings: &str, at: &str) -> rusqlite::Result<i64> {
    let Some(account_id) = account.id else { return Err(rusqlite::Error::QueryReturnedNoRows) };
    c.execute("INSERT INTO rules(user_id,type,status,exchange_id,asset_id,address,settings,created_at,updated_at) VALUES (?1,'Rules::Withdrawal',3,NULL,NULL,NULL,'{}',?2,?2)",
              params![account.user_id, at])?;
    let id = c.last_insert_rowid();
    c.execute(&format!("INSERT INTO exchange_account_rules(account_id,{COLUMNS}) VALUES (?1,?2,?3,'Rules::Withdrawal',0,?4,?5,?6,?7,?8,?8,?8)"),
              params![account_id, id, account.user_id, account.exchange_id, asset_id, address, settings, at])?;
    mirror(c, Some(id))?;
    Ok(id)
}

/// The takeover, after 2.x owned the install: the store's number-1 rules become 2.x's withdrawal rules again (ids kept;
/// one 2.x deleted goes). Run after identity::fill, so every (user, exchange) a rule names has its number 1; one that has
/// none fails the takeover (account_id NOT NULL) rather than losing the rule. A rule with no exchange stays 2.x's only, as
/// the migration leaves it.
pub fn refill(c: &Connection) -> rusqlite::Result<()> {
    const V2: &str = "FROM rules r WHERE r.type='Rules::Withdrawal' AND r.exchange_id IS NOT NULL";
    const NUMBER_1: &str = "(SELECT a.id FROM exchange_accounts a WHERE a.user_id=r.user_id AND a.exchange_id=r.exchange_id AND a.number=1)";
    c.execute(&format!("DELETE FROM exchange_account_rules WHERE account_id IN (SELECT id FROM exchange_accounts WHERE number=1) AND id NOT IN (SELECT r.id {V2})"), [])?;
    c.execute(&format!("INSERT INTO exchange_account_rules(account_id,{COLUMNS}) SELECT {NUMBER_1},r.id,r.user_id,r.type,r.status,r.exchange_id,r.asset_id,r.address,r.settings,r.settings_changed_at,r.created_at,r.updated_at \
                        {V2} AND NOT EXISTS (SELECT 1 FROM exchange_account_rules s JOIN exchange_accounts b ON b.id=s.account_id WHERE s.id=r.id AND b.number<>1) \
                        ON CONFLICT(id) DO UPDATE SET account_id=excluded.account_id,{UPDATED}"), [])?;
    Ok(())
}

/// The handback's refusal (handback_problems): a rule of an account 2+ has no place in 2.x's `rules`.
pub const HANDBACK_REFUSAL: &str = "a withdrawal rule of an exchange's second account is Rust-only";

pub fn table_exists(c: &Connection, name: &str) -> rusqlite::Result<bool> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |r| r.get(0),
    )
}
pub fn open_attempts(c: &Connection, rule: Option<i64>) -> Result<Vec<(i64, i64)>, EngineError> {
    let acks = if table_exists(c, "rust_v2_attempt_acks")? {
        "AND NOT EXISTS(SELECT 1 FROM rust_v2_attempt_acks a WHERE a.rule_log_id=l.id)"
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
    Ok(read_jobs(c, q)?.0)
}
/// The queue rows (solid_queue_jobs.id) of rule jobs this build cannot read: no active job id, or arguments that name no
/// withdrawal rule. Each is a takeover hard stop by its row id, so a malformed row never hides the readable ones.
pub fn unreadable_jobs(c: &Connection, q: &Connection) -> Result<Vec<i64>, EngineError> {
    Ok(read_jobs(c, q)?.1)
}
fn read_jobs(c: &Connection, q: &Connection) -> Result<(Vec<Job>, Vec<i64>), EngineError> {
    let mut s=q.prepare("SELECT j.active_job_id,j.class_name,j.arguments,EXISTS(SELECT 1 FROM solid_queue_claimed_executions e WHERE e.job_id=j.id) OR EXISTS(SELECT 1 FROM solid_queue_failed_executions e WHERE e.job_id=j.id),j.id FROM solid_queue_jobs j WHERE j.class_name IN ('Rule::EvaluateAllJob','Rule::ExecuteJob') AND j.finished_at IS NULL AND (EXISTS(SELECT 1 FROM solid_queue_claimed_executions e WHERE e.job_id=j.id) OR EXISTS(SELECT 1 FROM solid_queue_failed_executions e WHERE e.job_id=j.id) OR EXISTS(SELECT 1 FROM solid_queue_ready_executions e WHERE e.job_id=j.id) OR EXISTS(SELECT 1 FROM solid_queue_scheduled_executions e WHERE e.job_id=j.id) OR (j.class_name='Rule::ExecuteJob' AND EXISTS(SELECT 1 FROM solid_queue_blocked_executions e WHERE e.job_id=j.id))) ORDER BY j.id")?;
    let rows = s
        .query_map([], |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, bool>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let (mut jobs, mut unreadable) = (vec![], vec![]);
    for (id, class, args, claimed, row) in rows {
        let Some(id) = id else { unreadable.push(row); continue };
        let all = class == "Rule::EvaluateAllJob";
        let rule = if all {
            None
        } else {
            let a: Option<Value> = serde_json::from_str(args.as_deref().unwrap_or("")).ok();
            let gid = a.as_ref().map(|a| &a["arguments"][0]).and_then(|arg| arg["_aj_globalid"].as_str().or_else(|| arg.as_str())).unwrap_or("");
            let Some(rule) = gid.strip_prefix("gid://deltabadger/Rules::Withdrawal/").and_then(|s| s.parse::<i64>().ok()) else {
                unreadable.push(row);
                continue;
            };
            Some(rule)
        };
        let acknowledged = table_exists(c, "rust_v2_job_acks")?
            && c.query_row(
                "SELECT EXISTS(SELECT 1 FROM rust_v2_job_acks WHERE active_job_id=?1)",
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
    Ok((jobs, unreadable))
}
/// PLAN-B2 r4.7: a claimed or failed Rule::EvaluateAllJob only fanned out (its children are rule jobs of their own), so the
/// takeover acknowledges it as `resolve-withdrawal --v2-job <id> --skip` does, in the claim transaction. Returns the jobs
/// as acknowledged.
pub fn acknowledge_fanouts(c: &Connection, jobs: &mut [Job], now: DateTime<Utc>) -> Result<(), EngineError> {
    for job in jobs.iter_mut().filter(|j| j.all && j.claimed && !j.acknowledged) {
        c.execute("INSERT OR IGNORE INTO rust_v2_job_acks(active_job_id,rule_id,resolution,at) VALUES(?1,0,'skip',?2)", params![job.id, format_time(now)])?;
        job.acknowledged = true;
    }
    Ok(())
}
pub fn takeover_problems(c: &Connection, q: &Connection) -> Result<Vec<String>, EngineError> {
    let mut problems = open_attempts(c, None)?
        .into_iter()
        .map(|(id, rule)| {
            format!(
                "rule {rule}: 2.x attempt {id} is unconfirmed; resolve-withdrawal --v2 {id}"
            )
        })
        .collect::<Vec<_>>();
    // A claimed or failed fan-out is acknowledged at the claim (acknowledge_fanouts).
    for job in jobs(c, q)? {
        if job.claimed && !job.acknowledged && !job.all {
            problems.push(format!(
                "{} {}{} was claimed or failed; resolve-withdrawal --v2-job {}",
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
        let source = format!("v2:{}", job.id);
        added += if job.all {
            c.execute("INSERT OR IGNORE INTO rust_rule_evaluations(rule_id,source,created_at) SELECT id,?1,?2 FROM exchange_account_rules WHERE type='Rules::Withdrawal' AND status=1",params![source,format_time(now)])?
        } else {
            c.execute("INSERT OR IGNORE INTO rust_rule_evaluations(rule_id,source,created_at) SELECT id,?1,?2 FROM exchange_account_rules WHERE id=?3 AND type='Rules::Withdrawal'",params![source,format_time(now),job.rule])?
        };
        append_journal(c, IMPORT, &job.id, now)?;
    }
    seed_v2_ids(c, now)?;
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
    Ok(c.execute("INSERT INTO rust_rule_evaluations(rule_id,source,created_at) SELECT id,?1,?2 FROM exchange_account_rules WHERE type='Rules::Withdrawal' AND status=1",params![format!("start:{}",uuid::Uuid::new_v4()),format_time(now)])?)
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
        n=tx.execute("INSERT OR IGNORE INTO rust_rule_evaluations(rule_id,source,created_at) SELECT id,?1,?2 FROM exchange_account_rules WHERE type='Rules::Withdrawal' AND status=1",params![format!("fire:{}",at.to_rfc3339()),format_time(now)])?;
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
    if table_exists(c, "exchange_account_rules")?
        // #710 Merge: a deleted rule of a tombstoned account is history 2.x holds as its deleted placeholder.
        && c.query_row("SELECT EXISTS(SELECT 1 FROM exchange_account_rules r JOIN exchange_accounts a ON a.id=r.account_id WHERE a.number<>1 AND NOT (r.status=3 AND a.removed_at IS NOT NULL))", [], |r| r.get::<_, bool>(0))? {
        problems.push(HANDBACK_REFUSAL.into());
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
