//! Intent-first withdrawal execution. The only send follows a committed prepared -> dispatched CAS.
//! Recovery never sends. Unknown replies and absence (at any age) retain the attempt.
use super::{
    model::{self, CredentialVersion},
    rules, Clock, EngineError,
};
use crate::{
    crypto::Cipher,
    ruby::BigDec,
    venue::{withdrawal as v, VenueError},
};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

/// Real adapters join only with their E1-E3 G0 evidence. The live recorder uses this gate.
pub const G0_ADAPTERS: &[&str] = &["Exchanges::Kraken"];
pub const SEND_WINDOW_SECONDS: i64 = super::placement::SEND_WINDOW_SECONDS;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub host: String,
    pub account_scope: String,
    pub key_identity: String,
}
#[derive(Clone, Debug)]
pub enum SendResult {
    Accepted { id: String },
    Rejected { definitive: bool, reason: String },
    Ambiguous { reason: String },
}
#[derive(Clone, Debug)]
pub enum FoundStatus {
    Sent,
    Failed { reason: String },
}
#[derive(Clone, Debug)]
pub enum LookupResult {
    Found {
        id: String,
        status: FoundStatus,
    },
    NotFound,
    Unknown {
        reason: String,
        candidates: Vec<Value>,
    },
}
/// A handle is constructed with ONE credential snapshot. Implementations must not reload a key during I/O.
/// Found is an exact-client-id, full-parameter match; candidates belong in Unknown, never Found.
#[allow(async_fn_in_trait)]
pub trait Adapter {
    fn identity(&self) -> Identity;
    fn version(&self) -> CredentialVersion;
    fn fee_on_top(&self) -> Option<bool> {
        None
    }
    async fn balance(&self, coin: &str) -> Result<BigDec, VenueError>;
    async fn networks(&self, coin: &str) -> Result<Option<v::Coin>, VenueError>;
    async fn addresses(&self, coin: &str) -> Result<Vec<v::Entry>, VenueError>;
    async fn fee(
        &self,
        destination: &v::Destination,
        amount: &BigDec,
    ) -> Result<Option<BigDec>, VenueError>;
    async fn send(&self, request: &Request) -> SendResult;
    async fn lookup(&self, request: &Request) -> LookupResult;
}
#[derive(Clone, Debug)]
pub struct Request {
    pub rule_id: i64,
    pub user_id: i64,
    pub exchange_id: i64,
    pub asset_id: i64,
    pub exchange: String,
    pub symbol: String,
    pub coin: String,
    pub client_id: String,
    pub destination: v::Destination,
    pub amount: String,
    pub fee_cap: String,
    pub free_balance: String,
    pub percentage: String,
    pub created_at: String,
    pub wallet_type: Option<i64>,
}
impl Request {
    fn value(&self) -> Value {
        json!({"rule_id":self.rule_id,"user_id":self.user_id,"exchange_id":self.exchange_id,"asset_id":self.asset_id,"exchange":self.exchange,"symbol":self.symbol,"coin":self.coin,"client_id":self.client_id,"destination":destination_value(&self.destination),"amount":self.amount,"fee_cap":self.fee_cap,"free_balance":self.free_balance,"percentage":self.percentage,"created_at":self.created_at,"wallet_type":self.wallet_type})
    }
    fn read(value: Value) -> Result<Self, EngineError> {
        let text = |key: &str| {
            value[key]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| data("unreadable withdrawal request"))
        };
        let id = |key: &str| {
            value[key]
                .as_i64()
                .ok_or_else(|| data("unreadable withdrawal account"))
        };
        Ok(Self {
            rule_id: id("rule_id")?,
            user_id: id("user_id")?,
            exchange_id: id("exchange_id")?,
            asset_id: id("asset_id")?,
            exchange: text("exchange")?,
            symbol: text("symbol")?,
            coin: text("coin")?,
            client_id: text("client_id")?,
            destination: destination_read(&value["destination"])?,
            amount: text("amount")?,
            fee_cap: text("fee_cap")?,
            free_balance: text("free_balance")?,
            percentage: text("percentage")?,
            created_at: text("created_at")?,
            wallet_type: value["wallet_type"].as_i64(),
        })
    }
    fn stored(&self) -> v::Stored {
        match &self.destination {
            v::Destination::Binance {
                address,
                network,
                tag,
                ..
            } => v::Stored::new(address, Some(network), tag.as_deref(), None),
            v::Destination::Mexc {
                address,
                network,
                memo,
                ..
            } => v::Stored::new(address, Some(network), memo.as_deref(), None),
            v::Destination::Kraken { address, key, .. } => {
                v::Stored::new(address, None, None, Some(key))
            }
            v::Destination::Gemini {
                address, network, ..
            } => v::Stored::new(address, Some(network), None, None),
        }
    }
    fn details(&self, id: Option<&str>) -> Value {
        let s = self.stored();
        let mut v = json!({"free_balance":self.free_balance,"amount":self.amount,"fee":self.fee_cap,"address":s.address,"network":s.network,"address_tag":s.tag,"withdrawal_percentage":self.percentage});
        if let Some(id) = id {
            v["withdrawal_id"] = json!(id);
        }
        v
    }
}
fn destination_value(d: &v::Destination) -> Value {
    match d {
        v::Destination::Binance {
            coin,
            network,
            address,
            tag,
        } => json!({"kind":"Binance","coin":coin,"network":network,"address":address,"tag":tag}),
        v::Destination::Mexc {
            coin,
            network,
            net_work,
            address,
            memo,
        } => {
            json!({"kind":"Mexc","coin":coin,"network":network,"net_work":net_work,"address":address,"tag":memo})
        }
        v::Destination::Kraken {
            asset,
            key,
            address,
            method,
        } => json!({"kind":"Kraken","coin":asset,"key":key,"address":address,"method":method}),
        v::Destination::Gemini {
            ticker,
            network,
            address,
        } => json!({"kind":"Gemini","coin":ticker,"network":network,"address":address}),
    }
}
fn destination_read(d: &Value) -> Result<v::Destination, EngineError> {
    let s = |k: &str| {
        d[k].as_str()
            .map(str::to_owned)
            .ok_or_else(|| data("unreadable withdrawal destination"))
    };
    Ok(match s("kind")?.as_str() {
        "Binance" => v::Destination::Binance {
            coin: s("coin")?,
            network: s("network")?,
            address: s("address")?,
            tag: d["tag"].as_str().map(str::to_owned),
        },
        "Mexc" => v::Destination::Mexc {
            coin: s("coin")?,
            network: s("network")?,
            net_work: s("net_work")?,
            address: s("address")?,
            memo: d["tag"].as_str().map(str::to_owned),
        },
        "Kraken" => v::Destination::Kraken {
            asset: s("coin")?,
            key: s("key")?,
            address: s("address")?,
            // Older intents remain readable; lookup refuses to infer a missing method.
            method: d["method"].as_str().map(str::to_owned),
        },
        "Gemini" => v::Destination::Gemini {
            ticker: s("coin")?,
            network: s("network")?,
            address: s("address")?,
        },
        _ => return Err(data("unknown withdrawal destination")),
    })
}
fn data(s: &str) -> EngineError {
    EngineError::Data(s.into())
}
fn decimal(s: &str) -> Result<BigDec, EngineError> {
    BigDec::parse(s).map_err(|_| data("invalid withdrawal number"))
}
fn setting_number(settings: &Value, key: &str, default: &str) -> Result<BigDec, EngineError> {
    let value = &settings[key];
    if value.is_null() || value.as_str().is_some_and(crate::ruby::blank) {
        return decimal(default);
    }
    crate::ruby::json_to_d(value)
        .map_err(|_| data("invalid withdrawal number"))?
        .ok_or_else(|| data("invalid withdrawal number"))
}

#[derive(Clone, Debug)]
pub struct Intent {
    pub owner: String,
    pub state: String,
    pub client_id: String,
    pub request: Request,
    pub reason: Option<String>,
    version: CredentialVersion,
    identity: Identity,
}
pub fn load(c: &Connection, cipher: &Cipher, owner: &str) -> Result<Option<Intent>, EngineError> {
    let row=c.query_row("SELECT state,client_id,params_enc,api_key_id,credential_digest,host,account_scope,key_identity,reason FROM rust_withdrawal_intents WHERE owner=?1",[owner],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?,r.get::<_,Option<String>>(8)?))).optional()?;
    let Some((state, client_id, enc, id, digest, host, account_scope, key_identity, reason)) = row
    else {
        return Ok(None);
    };
    let raw = cipher
        .decrypt(&enc)
        .map_err(|_| data("withdrawal request cannot be decrypted"))?;
    let request = Request::read(
        serde_json::from_str(&raw).map_err(|_| data("unreadable withdrawal request"))?,
    )?;
    let version = CredentialVersion::from_stamp(&json!({"key_id":id,"ciphertext_digest":digest}))
        .ok_or_else(|| data("unreadable withdrawal credential version"))?;
    Ok(Some(Intent {
        owner: owner.into(),
        state,
        client_id,
        request,
        reason,
        version,
        identity: Identity {
            host,
            account_scope,
            key_identity,
        },
    }))
}
#[derive(Clone, PartialEq)]
struct Rule {
    id: i64,
    user: i64,
    exchange: i64,
    asset: i64,
    class: String,
    name: String,
    symbol: String,
    address: String,
    settings: Value,
    revision: String,
    status: i64,
}
fn rule(c: &Connection, cipher: &Cipher, id: i64) -> Result<Rule, EngineError> {
    let mut r=c.query_row("SELECT r.user_id,r.exchange_id,r.asset_id,e.type,e.name,a.symbol,r.address,r.settings,r.updated_at,r.status FROM rules r JOIN exchanges e ON e.id=r.exchange_id JOIN assets a ON a.id=r.asset_id WHERE r.id=?1 AND r.type='Rules::Withdrawal'",[id],|r|Ok(Rule{id,user:r.get(0)?,exchange:r.get(1)?,asset:r.get(2)?,class:r.get(3)?,name:r.get(4)?,symbol:r.get(5)?,address:r.get::<_,Option<String>>(6)?.unwrap_or_default(),settings:json!(r.get::<_,String>(7)?),revision:r.get(8)?,status:r.get(9)?}))?;
    r.address = cipher
        .decrypt(&r.address)
        .map_err(|_| data("withdrawal address cannot be decrypted"))?;
    r.settings = serde_json::from_str(r.settings.as_str().unwrap_or(""))
        .map_err(|_| data("unreadable withdrawal settings"))?;
    Ok(r)
}
// Called under the evaluation's immediate transaction. Rails ExecuteJob checks scheduled?
// before reading encrypted attributes and records unexpected rule-data failures per job.
fn evaluation_rule(
    c: &Connection,
    cipher: &Cipher,
    id: i64,
    evaluation: i64,
    now: DateTime<Utc>,
) -> Result<Option<Rule>, EngineError> {
    let status: i64 = c.query_row("SELECT status FROM rules WHERE id=?1", [id], |r| r.get(0))?;
    if status != 1 {
        consume(c, evaluation, "skipped", now)?;
        return Ok(None);
    }
    match rule(c, cipher, id) {
        Ok(r) => Ok(Some(r)),
        Err(EngineError::Data(message)) => {
            if consume(c, evaluation, "logged", now)? {
                rules::log(
                    c,
                    id,
                    None,
                    2,
                    &format!("Unexpected error: {message}"),
                    &json!({}),
                    now,
                )?;
            }
            Ok(None)
        }
        Err(error) => Err(error),
    }
}
fn key_current(
    c: &Connection,
    version: &CredentialVersion,
    user: i64,
    exchange: i64,
) -> Result<bool, EngineError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM api_keys WHERE id=?1 AND user_id=?2 AND exchange_id=?3 AND key_type=1 AND status=1)",params![version.id,user,exchange],|r|r.get::<_,bool>(0))?&&model::credential_is_current(c,version)?)
}
fn fence(c: &Connection, version: &CredentialVersion, r: &Rule) -> Result<(), EngineError> {
    model::locked(c, |tx| {
        if key_current(tx, version, r.user, r.exchange)? {
            Ok(())
        } else {
            Err(EngineError::CredentialsChanged)
        }
    })
}
fn same_key(c: &Connection, i: &Intent) -> Result<bool, EngineError> {
    key_current(c, &i.version, i.request.user_id, i.request.exchange_id)
}
fn matched(
    c: &Connection,
    cipher: &Cipher,
    i: &Intent,
    state: &str,
) -> Result<Option<Intent>, EngineError> {
    Ok(load(c, cipher, &i.owner)?.filter(|v| v.client_id == i.client_id && v.state == state))
}
fn remove(c: &Connection, i: &Intent, state: &str) -> Result<bool, EngineError> {
    let changed = c.execute(
        "DELETE FROM rust_withdrawal_intents WHERE owner=?1 AND client_id=?2 AND state=?3",
        params![i.owner, i.client_id, state],
    )? == 1;
    if changed {
        c.execute(
            "DELETE FROM rust_withdrawal_candidates WHERE owner=?1 AND client_id=?2",
            params![i.owner, i.client_id],
        )?;
    }
    Ok(changed)
}
fn uncertain(c: &Connection, i: &Intent, state: &str, reason: &str) -> Result<bool, EngineError> {
    Ok(c.execute("UPDATE rust_withdrawal_intents SET state='unresolved',reason=?4 WHERE owner=?1 AND client_id=?2 AND state=?3",params![i.owner,i.client_id,state,rules::safe_message(reason)])?==1)
}
fn consume(
    c: &Connection,
    evaluation: i64,
    outcome: &str,
    now: DateTime<Utc>,
) -> Result<bool, EngineError> {
    Ok(c.execute("UPDATE rust_rule_evaluations SET consumed_at=?1,outcome=?2 WHERE id=?3 AND consumed_at IS NULL",params![crate::codec::format_time(now),outcome,evaluation])?==1)
}
#[allow(clippy::too_many_arguments)] // The captured evaluation and Rails outcome are committed together.
fn outcome(
    c: &Connection,
    cipher: &Cipher,
    r: &Rule,
    evaluation: i64,
    version: Option<&CredentialVersion>,
    status: i64,
    message: &str,
    details: Value,
    now: DateTime<Utc>,
) -> Result<(), EngineError> {
    let tx = model::immediate(c)?;
    if let Some(v) = version {
        fence(&tx, v, r)?;
    }
    if rule(&tx, cipher, r.id)? != *r {
        return Err(EngineError::RunFenced);
    }
    if consume(&tx, evaluation, "logged", now)? {
        rules::log(&tx, r.id, None, status, message, &details, now)?;
    }
    tx.commit()?;
    Ok(())
}
fn read_error(e: VenueError, prefix: &str) -> (i64, String) {
    match e {
        VenueError::Transient(_) => (
            3,
            format!("{prefix} — temporary exchange issue, retrying on the next cycle"),
        ),
        other => {
            let message = match other {
                VenueError::Rejected(errors)
                | VenueError::Unauthorized(errors)
                | VenueError::Coded { errors, .. } => errors.first().cloned().unwrap_or_default(),
                VenueError::Ambiguous(message) | VenueError::Transient(message) => message,
            };
            (2, format!("{prefix}: {}", rules::safe_message(&message)))
        }
    }
}
/// Reads and begin. Returning an intent means it and evaluation consumption committed together; no send here.
pub async fn evaluate<A: Adapter>(
    c: &Connection,
    cipher: &Cipher,
    a: &A,
    evaluation: i64,
    clock: &dyn Clock,
) -> Result<Option<Intent>, EngineError> {
    let (r, credentials) = {
        let tx = model::immediate(c)?;
        let id: Option<i64> = tx
            .query_row(
                "SELECT rule_id FROM rust_rule_evaluations WHERE id=?1 AND consumed_at IS NULL",
                [evaluation],
                |r| r.get(0),
            )
            .optional()?;
        let Some(id) = id else { return Ok(None) };
        let Some(r) = evaluation_rule(&tx, cipher, id, evaluation, clock.now())? else {
            tx.commit()?;
            return Ok(None);
        };
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM rust_withdrawal_intents WHERE owner=?1)",
            [format!("rule:{id}")],
            |r| r.get::<_, bool>(0),
        )? {
            return Ok(None);
        }
        let credentials = model::withdrawal_credentials(&tx, cipher, r.user, r.exchange)?;
        tx.commit()?;
        (r, credentials)
    };
    let Some((credentials, version)) = credentials else {
        outcome(
            c,
            cipher,
            &r,
            evaluation,
            None,
            2,
            &format!(
                "Failed to fetch balance: No valid withdrawal API key for {}. Add one in Settings.",
                r.name
            ),
            json!({}),
            clock.now(),
        )?;
        return Ok(None);
    };
    if a.version() != version {
        return Err(EngineError::CredentialsChanged);
    }
    let identity = a.identity();
    let key = crate::venue::key_identity(&identity.host, &credentials);
    if key["key_fingerprint"] != identity.key_identity
        || identity.host.is_empty()
        || identity.account_scope.is_empty()
    {
        return Err(EngineError::CredentialsChanged);
    }
    let coin = v::venue_code(c, r.exchange, r.asset)?
        .ok_or_else(|| data("withdrawal asset is not listed"))?;
    let mut retries = 0;
    let free = loop {
        let result = a.balance(&coin).await;
        fence(c, &version, &r)?;
        match result {
            Ok(f) => break f,
            Err(VenueError::Transient(_)) if retries < 2 => {
                retries += 1;
            }
            Err(e) => {
                let (s, m) = read_error(e, "Failed to fetch balance");
                outcome(
                    c,
                    cipher,
                    &r,
                    evaluation,
                    Some(&version),
                    s,
                    &m,
                    json!({}),
                    clock.now(),
                )?;
                return Ok(None);
            }
        }
    };
    let networks = a.networks(&coin).await;
    fence(c, &version, &r)?;
    let networks = match networks {
        Ok(v) => v,
        Err(e) => {
            let (s, m) = read_error(e, "Failed to refresh withdrawal fees");
            outcome(
                c,
                cipher,
                &r,
                evaluation,
                Some(&version),
                s,
                &m,
                json!({"free_balance":free.to_s_f()}),
                clock.now(),
            )?;
            return Ok(None);
        }
    };
    let entries = a.addresses(&coin).await;
    fence(c, &version, &r)?;
    let kind = v::Kind::of(&r.class).ok_or_else(|| data("unsupported withdrawal venue"))?;
    let stored = v::Stored::new(
        &r.address,
        r.settings["network"].as_str(),
        r.settings["address_tag"].as_str(),
        r.settings["address_name"].as_str(),
    );
    let destination = entries
        .ok()
        .and_then(|entries| v::select(kind, &coin, &stored, &entries, networks.as_ref()).ok());
    let Some(destination) = destination else {
        outcome(
            c,
            cipher,
            &r,
            evaluation,
            Some(&version),
            2,
            "Withdrawal failed: withdrawal address is not listed",
            json!({"free_balance":free.to_s_f()}),
            clock.now(),
        )?;
        return Ok(None);
    };
    let pct = setting_number(&r.settings, "withdrawal_percentage", "100")?;
    if !pct.is_positive() || pct > BigDec::from_i64(100) || free < BigDec::zero() {
        return Err(data("invalid withdrawal budget"));
    }
    let multiple = if kind == v::Kind::Binance {
        networks
            .as_ref()
            .and_then(|n| {
                n.networks
                    .iter()
                    .find(|n| matches!(&destination,v::Destination::Binance{network,..} if *network==n.network))
            })
            .and_then(|n| n.multiple.as_ref())
    } else {
        None
    };
    let fraction = pct.div(&BigDec::from_i64(100)).expect("100");
    let budget = v::floor_p(&(&free * &fraction), multiple);
    let first = a.fee(&destination, &budget).await;
    fence(c, &version, &r)?;
    let first_fee = match first {
        Ok(Some(fee)) if fee >= BigDec::zero() => fee,
        Ok(_) => {
            outcome(
                c,
                cipher,
                &r,
                evaluation,
                Some(&version),
                0,
                &format!(
                    "Withdrawal fee unknown for {} — cannot evaluate fee percentage threshold",
                    r.symbol
                ),
                json!({"free_balance":free.to_s_f()}),
                clock.now(),
            )?;
            return Ok(None);
        }
        Err(e) => {
            let (s, m) = read_error(e, "Failed to refresh withdrawal fees");
            outcome(
                c,
                cipher,
                &r,
                evaluation,
                Some(&version),
                s,
                &m,
                json!({"free_balance":free.to_s_f()}),
                clock.now(),
            )?;
            return Ok(None);
        }
    };
    // Fees are refreshed for every evaluation, including one below the threshold.
    {
        let tx = model::immediate(c)?;
        fence(&tx, &version, &r)?;
        if rule(&tx, cipher, r.id)? != r {
            return Err(EngineError::RunFenced);
        }
        let stamp = networks
            .as_ref()
            .and_then(v::coin_stamp)
            .unwrap_or_else(|| v::quote_stamp(&first_fee));
        v::write_stamp(&tx, r.exchange, r.asset, &stamp, clock.now())?;
        tx.commit()?;
    }
    let fraction = pct.div(&BigDec::from_i64(100)).expect("nonzero divisor");
    let minimum = if r.settings["threshold_type"] == "min_amount" {
        setting_number(&r.settings, "min_amount", "0")?
            .div(&fraction)
            .expect("positive percentage")
            .round(8)
    } else {
        let max = setting_number(&r.settings, "max_fee_percentage", "0")?;
        if !max.is_positive() {
            return Err(data("invalid maximum withdrawal fee"));
        }
        first_fee
            .div(&max.div(&BigDec::from_i64(100)).expect("100"))
            .expect("positive max")
            .div(&fraction)
            .expect("positive fraction")
            .round(8)
    };
    let last: Option<String> = c.query_row(
        "SELECT max(created_at) FROM rule_logs WHERE rule_id=?1 AND status=1",
        [r.id],
        |r| r.get(0),
    )?;
    let interval = r.settings["max_interval"]
        .as_i64()
        .or_else(|| {
            r.settings["max_interval"]
                .as_str()
                .and_then(|s| s.parse::<i64>().ok())
        })
        .filter(|v| *v > 0);
    let last = last
        .as_deref()
        .map(crate::codec::parse_time)
        .transpose()
        .map_err(EngineError::Arithmetic)?;
    let interval = interval
        .map(|days| {
            chrono::Duration::try_days(days)
                .ok_or_else(|| data("withdrawal interval is out of range"))
        })
        .transpose()?;
    let bypass = interval.is_some_and(|duration| {
        last.is_none_or(|last| clock.now().signed_duration_since(last) > duration)
    });
    let below = if r.settings["threshold_type"] == "min_amount" {
        free < minimum
    } else {
        let max = setting_number(&r.settings, "max_fee_percentage", "0")?
            .div(&BigDec::from_i64(100))
            .expect("100");
        first_fee > &(&free * &fraction) * &max
    };
    if below && !bypass {
        outcome(
            c,
            cipher,
            &r,
            evaluation,
            Some(&version),
            0,
            &format!(
                "Balance {} {} below minimum {} {}",
                free.to_s_f(),
                r.symbol,
                minimum.to_s_f(),
                r.symbol
            ),
            json!({"free_balance":free.to_s_f()}),
            clock.now(),
        )?;
        return Ok(None);
    }
    let mut initial = Some(first_fee);
    let sized = v::size_with_semantics(&free, &pct, multiple, a.fee_on_top(), |amount| {
        let cached = initial.take();
        let (destination, version, r) = (&destination, &version, &r);
        async move {
            if let Some(fee) = cached {
                return Ok(Some(fee));
            }
            let result = a.fee(destination, &amount).await;
            if fence(c, version, r).is_err() {
                return Err(VenueError::Ambiguous(
                    "withdrawal credential changed".into(),
                ));
            }
            result
        }
    })
    .await;
    fence(c, &version, &r)?;
    let sized = match sized {
        Ok(s) => s,
        Err(e) => {
            let (s, m) = read_error(e, "Failed to refresh withdrawal fees");
            outcome(
                c,
                cipher,
                &r,
                evaluation,
                Some(&version),
                s,
                &m,
                json!({"free_balance":free.to_s_f()}),
                clock.now(),
            )?;
            return Ok(None);
        }
    };
    let (budget, amount, first_fee, fee_cap) = match sized {
        v::Sizing::FeeUnknown { .. } => {
            outcome(
                c,
                cipher,
                &r,
                evaluation,
                Some(&version),
                0,
                &format!(
                    "Withdrawal fee unknown for {} — cannot evaluate fee percentage threshold",
                    r.symbol
                ),
                json!({"free_balance":free.to_s_f()}),
                clock.now(),
            )?;
            return Ok(None);
        }
        v::Sizing::DoesNotCover { budget, fee } => {
            outcome(
                c,
                cipher,
                &r,
                evaluation,
                Some(&version),
                0,
                &format!(
                    "Withdrawal amount {} {} does not cover fee {} {}",
                    budget.to_s_f(),
                    r.symbol,
                    fee.to_s_f(),
                    r.symbol
                ),
                json!({"free_balance":free.to_s_f(),"withdrawal_percentage":pct.to_s_f()}),
                clock.now(),
            )?;
            return Ok(None);
        }
        v::Sizing::Send {
            budget,
            amount,
            first_fee,
            fee_cap,
        } => (budget, amount, first_fee, fee_cap),
    };
    let now = clock.now();
    let client_id = uuid::Uuid::new_v4().to_string();
    let request = Request {
        rule_id: r.id,
        user_id: r.user,
        exchange_id: r.exchange,
        asset_id: r.asset,
        exchange: r.class.clone(),
        symbol: r.symbol.clone(),
        coin,
        client_id: client_id.clone(),
        destination,
        amount: amount.to_s_f(),
        fee_cap: fee_cap.to_s_f(),
        free_balance: free.to_s_f(),
        percentage: pct.to_s_f(),
        created_at: crate::codec::format_time(now),
        wallet_type: (kind == v::Kind::Binance).then_some(0),
    };
    let owner = format!("rule:{}", r.id);
    let tx = model::immediate(c)?;
    model::check_credential_result(&tx, &Some(version.clone()))?;
    fence(&tx, &version, &r)?;
    if rule(&tx, cipher, r.id)? != r {
        return Err(EngineError::RunFenced);
    }
    if !consume(&tx, evaluation, "intent", now)? {
        return Ok(None);
    }
    if tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM rust_withdrawal_intents WHERE owner=?1)",
        [&owner],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(None);
    }
    tx.execute("INSERT INTO rust_withdrawal_intents(owner,evaluation_id,state,client_id,created_at,api_key_id,credential_digest,host,account_scope,key_identity,params_enc,fee_basis,budget,free_balance) VALUES(?1,?2,'prepared',?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",params![owner,evaluation,client_id,crate::codec::format_time(now),version.id,version.cache_stamp()["ciphertext_digest"].as_str(),identity.host,identity.account_scope,identity.key_identity,cipher.encrypt(&request.value().to_string()),json!({"first_fee":first_fee.to_s_f(),"fee_cap":fee_cap.to_s_f(),"fee_on_top":a.fee_on_top()}).to_string(),budget.to_s_f(),free.to_s_f()])?;
    let stamp = networks
        .as_ref()
        .and_then(v::coin_stamp)
        .unwrap_or_else(|| v::quote_stamp(&first_fee));
    v::write_stamp(&tx, r.exchange, r.asset, &stamp, now)?;
    tx.commit()?;
    load(c, cipher, &owner)
}
/// A guard failure deletes only the matching prepared request. A dispatched request cannot reach this path.
fn guard(
    c: &Connection,
    a: &impl Adapter,
    i: &Intent,
    now: DateTime<Utc>,
) -> Result<bool, EngineError> {
    let status: Option<(i64, i64, i64, i64)> = c
        .query_row(
            "SELECT status,user_id,exchange_id,asset_id FROM rules WHERE id=?1",
            [i.request.rule_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let stopped = status.is_none_or(|s| s.0 != 1);
    let account = status.is_some_and(|s| {
        s.1 == i.request.user_id && s.2 == i.request.exchange_id && s.3 == i.request.asset_id
    });
    let at = crate::codec::parse_time(&i.request.created_at).map_err(EngineError::Arithmetic)?;
    let expired = (now - at).num_seconds() > SEND_WINDOW_SECONDS || now < at;
    let current =
        same_key(c, i)? && a.identity() == i.identity && a.version() == i.version && account;
    if stopped || expired || !current {
        if remove(c, i, "prepared")? && !stopped {
            rules::log(
                c,
                i.request.rule_id,
                Some(&i.client_id),
                2,
                if !current {
                    "Withdrawal failed: the withdrawal key changed before sending"
                } else {
                    "Withdrawal failed: the request was not sent in time"
                },
                &i.request.details(None),
                now,
            )?;
        }
        return Ok(false);
    }
    Ok(true)
}

/// Validate the current rule against fresh listing data and commit the prepared -> dispatched CAS.
/// No venue send occurs here; the returned request is the immutable persisted one.
async fn prepare_dispatch<A: Adapter>(
    c: &Connection,
    cipher: &Cipher,
    a: &A,
    i: &Intent,
    clock: &dyn Clock,
) -> Result<Option<Intent>, EngineError> {
    let current = {
        let tx = model::immediate(c)?;
        let Some(current) = matched(&tx, cipher, i, "prepared")? else {
            return Ok(None);
        };
        if !guard(&tx, a, &current, clock.now())? {
            tx.commit()?;
            return Ok(None);
        }
        tx.commit()?;
        current
    };
    let networks = a.networks(&current.request.coin).await;
    // Do not read the listing with a handle whose credential snapshot has been replaced.
    let addresses = if same_key(c, &current)? {
        a.addresses(&current.request.coin).await.ok()
    } else {
        None
    };
    let tx = model::immediate(c)?;
    let Some(current) = matched(&tx, cipher, &current, "prepared")? else {
        return Ok(None);
    };
    if !guard(&tx, a, &current, clock.now())? {
        tx.commit()?;
        return Ok(None);
    }
    // Select from the rule as it is NOW, under the same lock as the CAS. Legacy blank
    // fields and network aliases must use the same normalization as evaluation.
    let r = rule(&tx, cipher, current.request.rule_id)?;
    let stored = v::Stored::new(
        &r.address,
        r.settings["network"].as_str(),
        r.settings["address_tag"].as_str(),
        r.settings["address_name"].as_str(),
    );
    let selected = networks.ok().zip(addresses).and_then(|(n, e)| {
        v::select(
            v::Kind::of(&r.class)?,
            &current.request.coin,
            &stored,
            &e,
            n.as_ref(),
        )
        .ok()
    });
    if r.class != current.request.exchange
        || selected.as_ref() != Some(&current.request.destination)
    {
        if remove(&tx, &current, "prepared")? {
            rules::log(
                &tx,
                r.id,
                Some(&current.client_id),
                2,
                "Withdrawal failed: withdrawal destination changed or is not listed",
                &current.request.details(None),
                clock.now(),
            )?;
        }
        tx.commit()?;
        return Ok(None);
    }
    model::check_credential_result(&tx, &Some(current.version.clone()))?;
    let changed = tx.execute("UPDATE rust_withdrawal_intents SET state='dispatched' WHERE owner=?1 AND client_id=?2 AND state='prepared'", params![current.owner, current.client_id])? == 1;
    tx.commit()?;
    Ok(changed.then_some(current))
}

pub async fn mark_dispatched(
    c: &Connection,
    cipher: &Cipher,
    a: &impl Adapter,
    i: &Intent,
    clock: &dyn Clock,
) -> Result<bool, EngineError> {
    Ok(prepare_dispatch(c, cipher, a, i, clock).await?.is_some())
}

pub async fn dispatch<A: Adapter>(
    c: &Connection,
    cipher: &Cipher,
    a: &A,
    i: &Intent,
    clock: &dyn Clock,
) -> Result<(), EngineError> {
    let Some(current) = prepare_dispatch(c, cipher, a, i, clock).await? else {
        return Ok(());
    };
    let result = a.send(&current.request).await;
    settle(c, cipher, &current, "dispatched", result, clock.now())?;
    Ok(())
}
/// Settlement is one conditional transaction. A stale answer has no writes, even to the id seed.
pub fn settle(
    c: &Connection,
    cipher: &Cipher,
    i: &Intent,
    state: &str,
    result: SendResult,
    now: DateTime<Utc>,
) -> Result<bool, EngineError> {
    settle_result(c, cipher, i, state, result, now, false)
}

// Only an exact-client-id lookup with a terminal failed status proves rejection independently of its text.
fn settle_result(
    c: &Connection,
    cipher: &Cipher,
    i: &Intent,
    state: &str,
    result: SendResult,
    now: DateTime<Utc>,
    failure_proven: bool,
) -> Result<bool, EngineError> {
    if !matches!(state, "dispatched" | "unresolved") {
        return Ok(false);
    }
    let tx = model::immediate(c)?;
    let Some(current) = matched(&tx, cipher, i, state)? else {
        return Ok(false);
    };
    if !same_key(&tx, &current)? {
        uncertain(&tx, &current, state, "the withdrawal key changed")?;
        tx.commit()?;
        return Ok(true);
    }
    let request = &current.request;
    match result {
        SendResult::Accepted { id } if !id.trim().is_empty() => {
            rules::seed_rails_ids(&tx)?;
            let inserted=tx.execute("INSERT OR IGNORE INTO rust_withdrawal_ids(exchange_id,venue_id,owner,settled_at,source) VALUES(?1,?2,?3,?4,'rust')",params![request.exchange_id,id,current.owner,crate::codec::format_time(now)])?;
            if inserted == 0 {
                uncertain(&tx, &current, state, "duplicate venue id")?;
            } else {
                rules::log(
                    &tx,
                    request.rule_id,
                    Some(&current.client_id),
                    1,
                    &format!("Withdrew {} {}", request.amount, request.symbol),
                    &request.details(Some(&id)),
                    now,
                )?;
                remove(&tx, &current, state)?;
            }
        }
        SendResult::Rejected {
            definitive: true,
            reason,
        } if failure_proven || rules::definitive(&request.exchange, &reason) => {
            rules::log(
                &tx,
                request.rule_id,
                Some(&current.client_id),
                2,
                &format!("Withdrawal failed: {}", rules::safe_message(&reason)),
                &request.details(None),
                now,
            )?;
            remove(&tx, &current, state)?;
        }
        SendResult::Rejected { reason, .. } | SendResult::Ambiguous { reason } => {
            uncertain(&tx, &current, state, &reason)?;
        }
        SendResult::Accepted { .. } => {
            uncertain(
                &tx,
                &current,
                state,
                "accepted reply without a withdrawal id",
            )?;
        }
    }
    tx.commit()?;
    Ok(true)
}
pub async fn recover<A: Adapter>(
    c: &Connection,
    cipher: &Cipher,
    a: &A,
    owner: &str,
    clock: &dyn Clock,
) -> Result<(), EngineError> {
    let current = {
        let tx = model::immediate(c)?;
        let Some(mut i) = load(&tx, cipher, owner)? else {
            return Ok(());
        };
        if i.state == "prepared" {
            remove(&tx, &i, "prepared")?;
            tx.commit()?;
            return Ok(());
        }
        note_unresolved(&i);
        if i.state == "dispatched" {
            uncertain(&tx, &i, "dispatched", "the reply was not recorded")?;
            i.state = "unresolved".into();
        }
        if !same_key(&tx, &i)? || (a.identity() != i.identity || a.version() != i.version) {
            uncertain(&tx, &i, "unresolved", "the withdrawal key changed")?;
            tx.commit()?;
            return Ok(());
        }
        tx.commit()?;
        i
    };
    let answer = a.lookup(&current.request).await;
    let tx = model::immediate(c)?;
    let Some(i) = matched(&tx, cipher, &current, "unresolved")? else {
        return Ok(());
    };
    if !same_key(&tx, &i)? || (a.identity() != i.identity || a.version() != i.version) {
        uncertain(&tx, &i, "unresolved", "the withdrawal key changed")?;
        tx.commit()?;
        return Ok(());
    }
    match answer {
        LookupResult::Found { id, status }
            if !id.trim().is_empty()
                && matches!(
                    v::Kind::of(&i.request.exchange),
                    Some(v::Kind::Binance | v::Kind::Mexc)
                ) =>
        {
            tx.commit()?;
            let result = match status {
                FoundStatus::Sent => SendResult::Accepted { id },
                FoundStatus::Failed { reason } => SendResult::Rejected {
                    definitive: true,
                    reason,
                },
            };
            settle_result(c, cipher, &i, "unresolved", result, clock.now(), true)?;
        }
        other => {
            let (reason, candidates) = match other {
                LookupResult::Unknown { reason, candidates } => (reason, candidates),
                LookupResult::NotFound => (
                    "no exact withdrawal found; absence is not proof".into(),
                    vec![],
                ),
                LookupResult::Found { id, .. } => (
                    "operator evidence required for this venue".into(),
                    vec![json!({"id":rules::safe_message(&id)})],
                ),
            };
            if uncertain(&tx, &i, "unresolved", &reason)? {
                let candidates = clean_candidates(candidates);
                tx.execute("INSERT INTO rust_withdrawal_candidates(owner,client_id,candidates) VALUES(?1,?2,?3) ON CONFLICT(owner) DO UPDATE SET client_id=excluded.client_id,candidates=excluded.candidates",params![i.owner,i.client_id,json!(candidates).to_string()])?;
            }
            tx.commit()?;
        }
    }
    Ok(())
}
fn note_unresolved(i: &Intent) {
    static SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    let key = format!("{}:{}", i.owner, i.client_id);
    if SEEN
        .get_or_init(Default::default)
        .lock()
        .expect("withdrawal log mutex")
        .insert(key)
    {
        super::log(&format!(
            "{}: withdrawal status unconfirmed; not retried; resolve-withdrawal {}",
            i.owner, i.request.rule_id
        ));
    }
}

fn clean_candidates(candidates: Vec<Value>) -> Vec<Value> {
    // Presentation is a bounded projection, never raw venue responses or credentials.
    candidates
        .into_iter()
        .take(20)
        .map(|v| {
            let mut out = json!({});
            for key in ["id", "status", "amount", "at"] {
                if let Some(s) = v[key].as_str() {
                    out[key] = json!(rules::safe_message(s));
                }
            }
            out
        })
        .collect()
}

#[derive(Clone, Debug)]
pub enum Target {
    Rule(i64),
    Rails(i64),
    RailsJob(String),
    RailsList(i64),
}
#[derive(Clone, Debug)]
pub enum Resolution {
    Sent { id: String, amount: Option<String> },
    NotSent,
    Recorded(i64),
    Skip,
    List,
}
pub fn parse_cli(args: &[String]) -> Result<(Target, Resolution), EngineError> {
    let usage = || {
        data("usage: resolve-withdrawal <rule_id> | --rails <rule_log_id> | --rails-job <job> (--sent <id> [--amount <amount>] | --not-sent | --recorded <rule_log_id> | --skip); --rails-list <rule_id>")
    };
    let id = |s: Option<&String>| {
        s.and_then(|v| v.parse::<i64>().ok())
            .filter(|n| *n > 0)
            .ok_or_else(usage)
    };
    let (target, rest) = match args.first().map(String::as_str) {
        Some("--rails-list") if args.len() == 2 => {
            return Ok((Target::RailsList(id(args.get(1))?), Resolution::List))
        }
        Some("--rails") => (Target::Rails(id(args.get(1))?), &args[2..]),
        Some("--rails-job") => (
            Target::RailsJob(
                args.get(1)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(usage)?
                    .clone(),
            ),
            &args[2..],
        ),
        Some(_) => (Target::Rule(id(args.first())?), &args[1..]),
        None => return Err(usage()),
    };
    let resolution = match rest
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--not-sent"] => Resolution::NotSent,
        ["--skip"] if matches!(target, Target::RailsJob(_)) => Resolution::Skip,
        ["--recorded", log] if matches!(target, Target::RailsJob(_)) => {
            Resolution::Recorded(log.parse().map_err(|_| usage())?)
        }
        ["--sent", id] if !id.trim().is_empty() => Resolution::Sent {
            id: (*id).into(),
            amount: None,
        },
        ["--sent", id, "--amount", amount]
            if !id.trim().is_empty() && decimal(amount).is_ok_and(|v| v.is_positive()) =>
        {
            Resolution::Sent {
                id: (*id).into(),
                amount: Some((*amount).into()),
            }
        }
        _ => return Err(usage()),
    };
    Ok((target, resolution))
}
/// The CLI holds the engine lock and migrates first. The caller commits the seed even on a duplicate-id refusal;
/// every other resolver write rolls back. No takeover is needed to make historical ids authoritative.
pub fn resolve(
    c: &Connection,
    q: &Connection,
    cipher: &Cipher,
    target: Target,
    resolution: Resolution,
    now: DateTime<Utc>,
) -> Result<(), EngineError> {
    let tx = model::immediate(c)?;
    rules::seed_rails_ids(&tx)?;
    if let Resolution::Sent { id, .. } = &resolution {
        let exchange=match &target{
   Target::Rule(id)=>load(&tx,cipher,&format!("rule:{id}"))?.ok_or_else(||data("no withdrawal intent"))?.request.exchange_id,
   Target::Rails(id)=>tx.query_row("SELECT r.exchange_id FROM rules r JOIN rule_logs l ON l.rule_id=r.id WHERE l.id=?1",[id],|r|r.get(0))?,
   Target::RailsJob(id)=>{let j=rules::jobs(&tx,q)?.into_iter().find(|j|j.id==*id).ok_or_else(||data("no such Rails job"))?;let rule=j.rule.ok_or_else(||data("EvaluateAllJob accepts --skip only"))?;tx.query_row("SELECT exchange_id FROM rules WHERE id=?1",[rule],|r|r.get(0))?},
   Target::RailsList(_)=>return Err(data("list accepts no resolution")),};
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM rust_withdrawal_ids WHERE exchange_id=?1 AND venue_id=?2)",
            params![exchange, id],
            |r| r.get(0),
        )?;
        if exists {
            tx.commit()?;
            return Err(data("withdrawal id already recorded for this exchange"));
        }
    }
    resolve_in(&tx, q, cipher, target, resolution, now)?;
    tx.commit()?;
    Ok(())
}
fn resolve_in(
    c: &Connection,
    q: &Connection,
    cipher: &Cipher,
    target: Target,
    resolution: Resolution,
    now: DateTime<Utc>,
) -> Result<(), EngineError> {
    let at = crate::codec::format_time(now);
    match target {
        Target::RailsList(_) => Err(data("use open_attempts to list Rails evidence")),
        Target::Rule(id) => {
            let i = load(c, cipher, &format!("rule:{id}"))?
                .ok_or_else(|| data("no withdrawal intent"))?;
            if !matches!(i.state.as_str(), "dispatched" | "unresolved") {
                return Err(data(
                    "only dispatched or unresolved attempts need resolution",
                ));
            }
            match resolution {
                Resolution::Sent { id, amount: _ } => {
                    insert_operator_id(c, i.request.exchange_id, &id, &i.owner, now)?;
                    rules::log(
                        c,
                        i.request.rule_id,
                        Some(&i.client_id),
                        1,
                        &format!("Withdrew {} {}", i.request.amount, i.request.symbol),
                        &i.request.details(Some(&id)),
                        now,
                    )?;
                }
                Resolution::NotSent => {
                    rules::log(
                        c,
                        i.request.rule_id,
                        Some(&i.client_id),
                        2,
                        "Withdrawal failed: operator confirmed not sent",
                        &i.request.details(None),
                        now,
                    )?;
                }
                _ => return Err(data("invalid intent resolution")),
            }
            remove(c, &i, &i.state)?;
            Ok(())
        }
        Target::Rails(log) => {
            let rule = rules::open_attempts(c, None)?
                .into_iter()
                .find(|(id, _)| *id == log)
                .map(|(_, rule)| rule)
                .ok_or_else(|| data("not an open ambiguous Rails attempt"))?;
            let raw: String =
                c.query_row("SELECT details FROM rule_logs WHERE id=?1", [log], |r| {
                    r.get(0)
                })?;
            let details: Value =
                serde_json::from_str(&raw).map_err(|_| data("unreadable Rails attempt details"))?;
            let (name, venue) = resolve_log(c, rule, &resolution, details, now)?;
            c.execute("INSERT INTO rust_rails_attempt_acks(rule_log_id,rule_id,resolution,venue_id,at) VALUES(?1,?2,?3,?4,?5)",params![log,rule,name,venue,at])?;
            Ok(())
        }
        Target::RailsJob(job) => {
            let j = rules::jobs(c, q)?
                .into_iter()
                .find(|j| j.id == job)
                .ok_or_else(|| data("no such Rails withdrawal job"))?;
            if j.acknowledged {
                return Err(data("Rails job already acknowledged"));
            }
            if j.all {
                if !j.claimed || !matches!(resolution, Resolution::Skip) {
                    return Err(data(
                        "claimed EvaluateAllJob requires --skip; no evaluations are imported",
                    ));
                }
                c.execute("INSERT INTO rust_rails_job_acks(active_job_id,rule_id,resolution,at) VALUES(?1,0,'skip',?2)",params![job,at])?;
            } else {
                let rule = j.rule.ok_or_else(|| data("job has no rule"))?;
                let (name, venue) = if let Resolution::Recorded(log) = resolution {
                    let (source_rule, status, raw): (i64, i64, String) = c.query_row(
                        "SELECT rule_id,status,details FROM rule_logs WHERE id=?1",
                        [log],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )?;
                    if source_rule != rule {
                        return Err(data("recorded evidence belongs to another rule"));
                    }
                    let details: Value = serde_json::from_str(&raw)
                        .map_err(|_| data("unreadable recorded evidence"))?;
                    let success = status == 1
                        && (details["withdrawal_id"].is_number()
                            || details["withdrawal_id"]
                                .as_str()
                                .is_some_and(|s| !s.is_empty()));
                    let ack:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM rust_rails_attempt_acks WHERE rule_log_id=?1 AND rule_id=?2)",params![log,rule],|r|r.get(0))?;
                    if !success
                        && !ack
                        && !rules::open_attempts(c, Some(rule))?
                            .iter()
                            .any(|(id, _)| *id == log)
                    {
                        return Err(data(
                            "--recorded requires success or ambiguous attempt evidence",
                        ));
                    }
                    // Link evidence, never insert a second id or manufacture a success row.
                    (format!("recorded:{log}"), None)
                } else {
                    resolve_log(c, rule, &resolution, json!({}), now)?
                };
                c.execute("INSERT INTO rust_rails_job_acks(active_job_id,rule_id,resolution,venue_id,at) VALUES(?1,?2,?3,?4,?5)",params![job,rule,name,venue,at])?;
            }
            rules::append_journal(c, rules::CLEANUP, &job, now)?;
            Ok(())
        }
    }
}
fn insert_operator_id(
    c: &Connection,
    exchange: i64,
    id: &str,
    owner: &str,
    now: DateTime<Utc>,
) -> Result<(), EngineError> {
    if id.trim().is_empty() {
        return Err(data("withdrawal id must not be empty"));
    }
    rules::seed_rails_ids(c)?;
    c.execute("INSERT INTO rust_withdrawal_ids(exchange_id,venue_id,owner,settled_at,source) VALUES(?1,?2,?3,?4,'operator')",params![exchange,id,owner,crate::codec::format_time(now)])?;
    Ok(())
}
fn resolve_log(
    c: &Connection,
    rule: i64,
    resolution: &Resolution,
    mut details: Value,
    now: DateTime<Utc>,
) -> Result<(String, Option<String>), EngineError> {
    match resolution {
        Resolution::Sent { id, amount } => {
            let amount = amount
                .clone()
                .or_else(|| details["amount"].as_str().map(str::to_owned))
                .ok_or_else(|| data("--amount is required when the Rails attempt has no amount"))?;
            if !decimal(&amount)?.is_positive() {
                return Err(data("amount must be positive"));
            }
            let (exchange,symbol):(i64,String)=c.query_row("SELECT r.exchange_id,a.symbol FROM rules r JOIN assets a ON a.id=r.asset_id WHERE r.id=?1",[rule],|r|Ok((r.get(0)?,r.get(1)?)))?;
            insert_operator_id(c, exchange, id, &format!("rule:{rule}"), now)?;
            details["amount"] = json!(amount);
            details["withdrawal_id"] = json!(id);
            rules::log(
                c,
                rule,
                None,
                1,
                &format!("Withdrew {amount} {symbol}"),
                &details,
                now,
            )?;
            Ok(("sent".into(), Some(id.clone())))
        }
        Resolution::NotSent => {
            rules::log(
                c,
                rule,
                None,
                2,
                "Withdrawal failed: operator confirmed not sent",
                &details,
                now,
            )?;
            Ok(("not-sent".into(), None))
        }
        _ => Err(data("invalid Rails attempt resolution")),
    }
}

/// Serial engine phase. Production sends remain gated by the registry; no adapter is enabled in E0.
/// Even on disabled venues, prepared crash remnants are dropped and stopped evaluations are consumed.
pub async fn phase<F: crate::venue::VenueFactory>(
    c: &Connection,
    cipher: &Cipher,
    factory: &F,
    clock: &dyn Clock,
) -> Result<(), EngineError> {
    if rules::journals_pending(c)? {
        return Ok(());
    }
    rules::fire(c, clock.now())?;
    let owners = c
        .prepare("SELECT owner FROM rust_withdrawal_intents ORDER BY created_at,owner")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for owner in owners {
        let Some(i) = load(c, cipher, &owner)? else {
            continue;
        };
        if i.state == "prepared" {
            let tx = model::immediate(c)?;
            remove(&tx, &i, "prepared")?;
            tx.commit()?;
            continue;
        }
        let credentials =
            model::withdrawal_credentials(c, cipher, i.request.user_id, i.request.exchange_id)?;
        let Some((cr, version)) = credentials else {
            let tx = model::immediate(c)?;
            uncertain(&tx, &i, &i.state, "the withdrawal key changed")?;
            tx.commit()?;
            continue;
        };
        let adapter = VenueAdapter {
            venue: factory.for_bot(&i.request.exchange, Some(cr)),
            fee_on_top: crate::venue::registry::row(&i.request.exchange)
                .and_then(|row| row.fee_on_top),
            version,
        };
        recover(c, cipher, &adapter, &owner, clock).await?;
    }
    let evaluations = c
        .prepare(
            "SELECT id,rule_id FROM rust_rule_evaluations WHERE consumed_at IS NULL ORDER BY id",
        )?
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    for (evaluation, id) in evaluations {
        let r = {
            let tx = model::immediate(c)?;
            let r = evaluation_rule(&tx, cipher, id, evaluation, clock.now())?;
            tx.commit()?;
            r
        };
        let Some(r) = r else { continue };
        if !crate::venue::registry::row(&r.class).is_some_and(|v| v.withdraws) {
            continue;
        }
        let Some((cr, version)) = model::withdrawal_credentials(c, cipher, r.user, r.exchange)?
        else {
            outcome(c,cipher,&r,evaluation,None,2,&format!("Failed to fetch balance: No valid withdrawal API key for {}. Add one in Settings.",r.name),json!({}),clock.now())?;
            continue;
        };
        let adapter = VenueAdapter {
            venue: factory.for_bot(&r.class, Some(cr)),
            fee_on_top: crate::venue::registry::row(&r.class).and_then(|row| row.fee_on_top),
            version,
        };
        if let Some(i) = evaluate(c, cipher, &adapter, evaluation, clock).await? {
            dispatch(c, cipher, &adapter, &i, clock).await?;
        }
    }
    Ok(())
}
/// One credential snapshot for V1 reads and the venue withdrawal send/lookup implementation.
pub struct VenueAdapter<V> {
    venue: V,
    version: CredentialVersion,
    fee_on_top: Option<bool>,
}
impl<V> VenueAdapter<V> {
    pub fn new(venue: V, version: CredentialVersion, fee_on_top: Option<bool>) -> Self {
        Self { venue, version, fee_on_top }
    }
}
impl<V: crate::venue::Venue> Adapter for VenueAdapter<V> {
    fn fee_on_top(&self) -> Option<bool> {
        self.fee_on_top
    }
    fn version(&self) -> CredentialVersion {
        self.version.clone()
    }
    fn identity(&self) -> Identity {
        let v = self.venue.placement_identity().unwrap_or(Value::Null);
        Identity {
            host: v["host"].as_str().unwrap_or("").into(),
            account_scope: "spot".into(),
            key_identity: v["key_fingerprint"].as_str().unwrap_or("").into(),
        }
    }
    async fn balance(&self, coin: &str) -> Result<BigDec, VenueError> {
        self.venue.balance(coin, true).await
    }
    async fn networks(&self, coin: &str) -> Result<Option<v::Coin>, VenueError> {
        self.venue.withdrawal_networks(coin).await
    }
    async fn addresses(&self, coin: &str) -> Result<Vec<v::Entry>, VenueError> {
        self.venue.withdrawal_addresses(coin).await
    }
    async fn fee(&self, d: &v::Destination, amount: &BigDec) -> Result<Option<BigDec>, VenueError> {
        self.venue.withdrawal_fee(d, amount).await
    }
    async fn send(&self, request: &Request) -> SendResult {
        self.venue.withdraw(request).await
    }
    async fn lookup(&self, request: &Request) -> LookupResult {
        self.venue.withdrawal_lookup(request).await
    }
}
