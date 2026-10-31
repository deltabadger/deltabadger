//! Withdrawal reads (PLAN-withdrawal-rules V1): the venues' address books, the §4 destination contract a rule is held to,
//! the fee reads and quotes, the §6 sizing, and shared E1 send/recovery validation.
//!
//! Rails is the oracle where it is right (tests/fixtures/withdrawal_vectors.json `reads`, recorded through honeymaker
//! 0.12.5): Binance's and Kraken's listing requests and filters (the coin, Kraken's `verified`), Gemini's NETWORK_MAP and
//! `active`, and the coin list's default network and chain shape. Where Rails is wrong the port diverges, by name:
//! - D-RULE-LISTED-ENTRY: an entry is the full destination (address, network, tag; Kraken's key). Rails keeps the address
//!   only and sends a rule's own network and free-form tag beside it. A rule's stored network, tag and key must equal the
//!   entry's; a field the rule leaves empty is taken from the one entry that matches, never inferred; two distinct matches
//!   are refused as ambiguous.
//! - D-RULE-MEXC-LISTING: MEXC answers a page object `{data, totalRecords, page, totalPageNum}`; Rails' `Array(result.data)`
//!   raises TypeError on it. Every page is read as `data[]`.
//! - D-RULE-MEXC-ADDRESSTAG: an entry's memo is its `memo` only; one with an `addressTag` and no `memo` is refused.
//! - D-RULE-GEMINI-MEMO: Gemini lists through Rails' NETWORK_MAP, but only the memo-free networks are a destination.
//! - D-RULE-KRAKEN-QUOTE: Kraken's fee is a `WithdrawInfo` quote for the exact amount (Rails reads WithdrawMethods' first
//!   method and drops percentage fees); a quote whose net is not `amount − fee` is fee unknown.
//! - D-RULE-GEMINI-FEE-ESTIMATE: Gemini's fee is `feeEstimate` (Rails knows no Gemini fee).
//! - D-RULE-FEE-FALLBACK (Q1): the rules page's fee display reads Binance's and MEXC's coin list with the FeeApiKey, else
//!   the rule's own withdrawal key. The engine always sizes with the withdrawal key: its account pays the fee and is the
//!   identity fenced at begin and dispatch. Rails reads nothing without a FeeApiKey (nothing in the app creates one), so
//!   its fee stays unknown.
//! - D-RULE-FEE-STAMP: a fee read is written to the rule's own exchange_assets row only, and only when it is known. Rails
//!   writes every coin the venue lists (creating rows), and stamps a missing or unreadable fee as fresh for 24 hours.
//! - D-RULE-LOG-SECRETS: no error text this module returns carries a URL query, a signature or an HTML body.
use super::VenueError;
use crate::ruby::BigDec;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

/// The refusal every surface shows for a destination the contract does not admit (§4).
pub const NOT_LISTED_KEY: &str = "errors.withdrawal_address_not_listed";
/// Binance's (and MEXC's) honeymaker recvWindow for the wallet reads, as Rails sends it.
pub const RECV_WINDOW: &str = "5000";
pub const BINANCE_ADDRESSES: &str = "/sapi/v1/capital/withdraw/address/list";
pub const BINANCE_COINS: &str = "/sapi/v1/capital/config/getall";
pub const MEXC_ADDRESSES: &str = "/api/v3/capital/withdraw/address";
pub const MEXC_COINS: &str = "/api/v3/capital/config/getall";
pub const KRAKEN_ADDRESSES: &str = "/0/private/WithdrawAddresses";
pub const KRAKEN_INFO: &str = "/0/private/WithdrawInfo";
/// MEXC's page size for the address book, and the most pages read before the listing is refused as too long.
pub const MEXC_PAGE: usize = 50;
pub const MEXC_MAX_PAGES: i64 = 100;

/// Exchanges::Gemini::NETWORK_MAP: the approvedAddresses network each ticker lists under.
pub const GEMINI_NETWORK_MAP: [(&str, &str); 21] = [("BTC", "bitcoin"), ("ETH", "ethereum"), ("LTC", "litecoin"), ("SOL", "solana"),
    ("BCH", "bitcoincash"), ("DOGE", "dogecoin"), ("FIL", "filecoin"), ("MATIC", "polygon"), ("DOT", "polkadot"), ("XRP", "ripple"),
    ("XLM", "stellar"), ("AVAX", "avalanche"), ("ADA", "cardano"), ("ATOM", "cosmos"), ("ALGO", "algorand"), ("NEAR", "near"),
    ("APT", "aptos"), ("SUI", "sui"), ("XTZ", "tezos"), ("EOS", "eos"), ("LINK", "chainlink")];
/// D-RULE-GEMINI-MEMO: the networks whose withdrawal needs no memo (plan §4); NETWORK_MAP ∩ this set are destinations.
pub const GEMINI_MEMO_FREE: [&str; 10] = ["bitcoin", "ethereum", "bitcoincash", "litecoin", "zcash", "filecoin", "dogecoin", "tezos", "polkadot", "avalanche"];

pub fn gemini_network(ticker: &str) -> Option<&'static str> {
    GEMINI_NETWORK_MAP.iter().find(|(t, _)| t.eq_ignore_ascii_case(ticker)).map(|(_, n)| *n)
}

/// The venues that withdraw (exchange.rb:534 less Binance.US, D-RULE-BINANCE-US-UNSUPPORTED: it has no address book, so no
/// destination contract).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Binance, Kraken, Gemini, Mexc }

impl Kind {
    pub fn of(exchange_type: &str) -> Option<Self> {
        match exchange_type {
            super::binance::BINANCE => Some(Self::Binance),
            "Exchanges::Kraken" => Some(Self::Kraken),
            super::gemini::GEMINI => Some(Self::Gemini),
            super::mexc::MEXC => Some(Self::Mexc),
            _ => None,
        }
    }
}

/// One address-book entry as the venue lists it. `refused` names why the contract does not admit it (unverified,
/// inactive, no network, a MEXC addressTag without a memo); such an entry is never selected.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub address: String,
    pub network: Option<String>,
    /// Binance's addressTag, MEXC's memo.
    pub tag: Option<String>,
    /// Kraken's key name.
    pub key: Option<String>,
    pub label: Option<String>,
    pub refused: Option<&'static str>,
}

/// §4: what a withdrawal is sent to, every field present and unambiguous.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    Binance { coin: String, network: String, address: String, tag: Option<String> },
    Kraken { asset: String, key: String, address: String, method: Option<String> },
    Gemini { ticker: String, network: String, address: String },
    /// `network` is the coin list's label (the old endpoint's), `net_work` the new endpoint's code (D-RULE-MEXC-CAPITAL).
    Mexc { coin: String, network: String, net_work: String, address: String, memo: Option<String> },
}

/// A rule's stored destination: `address`, settings `network`, `address_tag` and `address_name` (Kraken's key). Blank is
/// absent, as Rails reads them with `.presence`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stored { pub address: String, pub network: Option<String>, pub tag: Option<String>, pub key: Option<String> }

impl Stored {
    pub fn new(address: &str, network: Option<&str>, tag: Option<&str>, key: Option<&str>) -> Self {
        let present = |v: Option<&str>| v.filter(|s| !crate::ruby::blank(s)).map(str::to_string);
        Self { address: address.to_string(), network: present(network), tag: present(tag), key: present(key) }
    }
}

/// Why a stored destination is not executable (§4). Every surface shows NOT_LISTED_KEY for each.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    NotListed,
    /// More than one distinct listed destination matches what the rule stores.
    Ambiguous,
    /// The listed entry exists but the contract does not admit it (its reason).
    Inadmissible(&'static str),
}

/// One network of a coin (Binance's and MEXC's capital/config/getall).
#[derive(Clone, Debug, PartialEq)]
pub struct Network {
    pub network: String,
    /// MEXC's netWork (the new withdraw endpoint's code); None on Binance.
    pub net_work: Option<String>,
    pub withdraw_enable: bool,
    /// Binance's withdrawTag (a memo is required); None when absent.
    pub withdraw_tag: Option<bool>,
    /// The withdrawFee, None when absent, unreadable or negative.
    pub fee: Option<BigDec>,
    pub fee_raw: Value,
    pub is_default: bool,
    /// withdrawIntegerMultiple, when positive.
    pub multiple: Option<BigDec>,
}

/// A coin's networks, in the venue's order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Coin { pub networks: Vec<Network> }

impl Coin {
    /// The network Rails reads a coin's fee from: the first `isDefault`, else the first.
    pub fn default_network(&self) -> Option<&Network> { self.networks.iter().find(|n| n.is_default).or_else(|| self.networks.first()) }
}

pub(crate) fn not_connected() -> VenueError { VenueError::Rejected(vec!["withdrawals are not connected for this venue".into()]) }
fn unreadable(venue: &str) -> VenueError { VenueError::Ambiguous(format!("{venue}: unreadable withdrawal answer")) }
fn text(v: &Value) -> Option<String> { v.as_str().filter(|s| !crate::ruby::blank(s)).map(str::to_string) }
/// A fee: a finite decimal ≥ 0, else unknown.
pub fn known_fee(v: &Value) -> Option<BigDec> { crate::ruby::json_to_d(v).ok().flatten().filter(|f| *f >= BigDec::zero()) }

// ---- D-RULE-LOG-SECRETS

/// An error text safe to store or show: an HTML body is named, never kept; a URL keeps its host and path (the signed
/// query — address, amount, timestamp, signature — is cut); the whole is capped at 200 characters.
pub fn log_safe(message: &str) -> String {
    if message.trim_start().starts_with('<') { return "the venue answered with an HTML page".into(); }
    let url = regex::Regex::new(r#"https?://[^\s"'<>]+"#).expect("static regex");
    let cut = url.replace_all(message, |c: &regex::Captures<'_>| c[0].split(['?', '#']).next().unwrap_or_default().to_string());
    let mut out: String = cut.chars().take(200).collect();
    if cut.chars().count() > 200 { out.push('…'); }
    out
}

/// `log_safe` on every text a venue error carries.
pub fn scrub(e: VenueError) -> VenueError {
    let all = |v: Vec<String>| v.iter().map(|m| log_safe(m)).collect();
    match e {
        VenueError::Rejected(v) => VenueError::Rejected(all(v)),
        VenueError::Unauthorized(v) => VenueError::Unauthorized(all(v)),
        VenueError::Coded { code, errors } => VenueError::Coded { code, errors: all(errors) },
        VenueError::Ambiguous(m) => VenueError::Ambiguous(log_safe(&m)),
        VenueError::Transient(m) => VenueError::Transient(log_safe(&m)),
    }
}

// ---- listings

/// Exchanges::Binance#list_withdrawal_addresses' rows for `coin`, keeping what Rails drops: the network and addressTag
/// (D-RULE-LISTED-ENTRY). An entry with no network is refused.
pub fn binance_entries(body: &Value, coin: &str) -> Result<Vec<Entry>, VenueError> {
    let rows = body.as_array().ok_or_else(|| unreadable("Binance"))?;
    Ok(rows.iter().filter(|r| r["coin"].as_str() == Some(coin)).filter_map(|r| {
        let address = text(&r["address"])?;
        let network = text(&r["network"]);
        Some(Entry { refused: network.is_none().then_some("the entry names no network"), address, network, tag: text(&r["addressTag"]),
                     key: None, label: text(&r["name"]) })
    }).collect())
}

/// Exchanges::Kraken#list_withdrawal_addresses: the envelope checked, an unverified entry refused.
pub fn kraken_entries(body: &Value) -> Result<Vec<Entry>, VenueError> {
    super::kraken::check(body)?;
    let rows = body["result"].as_array().ok_or_else(|| unreadable("Kraken"))?;
    Ok(rows.iter().filter_map(|r| {
        let address = text(&r["address"])?;
        let key = text(&r["key"]);
        let refused = if r["verified"] != Value::Bool(true) { Some("the entry is not verified") } else if key.is_none() { Some("the entry names no key") } else { None };
        Some(Entry { address, network: text(&r["method"]), tag: None, key, label: None, refused })
    }).collect())
}

/// Exchanges::Gemini#list_withdrawal_addresses: `approvedAddresses`, an entry that is not `active` refused. Gemini lists
/// one address once per scope (account, group): the same destination twice.
pub fn gemini_entries(body: &Value) -> Result<Vec<Entry>, VenueError> {
    let rows = body["approvedAddresses"].as_array().ok_or_else(|| unreadable("Gemini"))?;
    Ok(rows.iter().filter_map(|r| {
        let address = text(&r["address"])?;
        Some(Entry { refused: (r["status"].as_str() != Some("active")).then_some("the address is not active"), address, network: text(&r["network"]),
                     tag: None, key: None, label: text(&r["label"]) })
    }).collect())
}

/// One MEXC address-book page (D-RULE-MEXC-LISTING): its `data[]` rows for `coin` and its `totalPageNum`. The memo is
/// `memo` only; an `addressTag` without a `memo` is refused (D-RULE-MEXC-ADDRESSTAG).
pub fn mexc_page(body: &Value, coin: &str) -> Result<(Vec<Entry>, i64), VenueError> {
    let rows = body["data"].as_array().ok_or_else(|| unreadable("MEXC"))?;
    let pages = body["totalPageNum"].as_i64().ok_or_else(|| unreadable("MEXC"))?;
    let entries = rows.iter().filter(|r| r["coin"].as_str() == Some(coin)).filter_map(|r| {
        let address = text(&r["address"])?;
        let (network, memo, tag) = (text(&r["network"]), text(&r["memo"]), text(&r["addressTag"]));
        let refused = if network.is_none() { Some("the entry names no network") }
                      else if tag.is_some() && memo.is_none() { Some("the entry has an addressTag and no memo") } else { None };
        Some(Entry { address, network, tag: memo, key: None, label: None, refused })
    }).collect();
    Ok((entries, pages))
}

/// capital/config/getall's networks for `coin` (Binance and MEXC alike). None when the coin is not listed.
pub fn coin_networks(body: &Value, coin: &str, venue: &str) -> Result<Option<Coin>, VenueError> {
    let coins = body.as_array().ok_or_else(|| unreadable(venue))?;
    let Some(row) = coins.iter().find(|c| c["coin"].as_str() == Some(coin)) else { return Ok(None) };
    let list = match &row["networkList"] { Value::Null => &[][..], Value::Array(a) => a.as_slice(), _ => return Err(unreadable(venue)) };
    let networks = list.iter().map(|n| Network {
        network: n["network"].as_str().unwrap_or_default().to_string(),
        net_work: text(&n["netWork"]),
        withdraw_enable: n["withdrawEnable"] == Value::Bool(true),
        withdraw_tag: n["withdrawTag"].as_bool(),
        fee: known_fee(&n["withdrawFee"]),
        fee_raw: n["withdrawFee"].clone(),
        is_default: n["isDefault"] == Value::Bool(true),
        multiple: known_fee(&n["withdrawIntegerMultiple"]).filter(BigDec::is_positive),
    }).collect();
    Ok(Some(Coin { networks }))
}

// ---- the §4 contract

/// The one destination a rule's stored fields select from the listing, or why none can be. `coin` is the venue's code
/// for the asset (`venue_code`); Binance and MEXC need the coin's networks.
pub fn select(kind: Kind, coin: &str, stored: &Stored, entries: &[Entry], networks: Option<&Coin>) -> Result<Destination, Refusal> {
    let same = |a: &Option<String>, b: &Option<String>| a.is_none() || a == b;
    let at_address: Vec<&Entry> = entries.iter().filter(|e| e.address == stored.address).collect();
    let matching: Vec<&Entry> = at_address.iter().copied().filter(|e| match kind {
        Kind::Kraken => same(&stored.key, &e.key),
        // Gemini's scoped endpoint supplies the network when the entry omits it.
        // destination() checks both the stored and explicit listed network against that scope.
        Kind::Mexc | Kind::Gemini => same(&stored.tag, &e.tag),
        _ => same(&stored.network, &e.network) && same(&stored.tag, &e.tag),
    }).collect();
    let admitted: Vec<&Entry> = matching.iter().copied().filter(|e| e.refused.is_none()).collect();
    if admitted.is_empty() {
        // An entry at the address the contract refuses is named (a MEXC addressTag the rule stored, say), else not listed.
        let reason = matching.iter().chain(at_address.iter()).find_map(|e| e.refused);
        return Err(reason.map_or(Refusal::NotListed, Refusal::Inadmissible));
    }
    let mut picked: Vec<Destination> = vec![];
    for e in admitted {
        // An entry on a network other than the stored one drops out before the pick: only the rest can be ambiguous.
        let d = match destination(kind, coin, stored, e, networks) { Err(Refusal::NotListed) => continue, d => d? };
        if !picked.contains(&d) { picked.push(d); }
    }
    if picked.len() > 1 { return Err(Refusal::Ambiguous); }
    if picked.is_empty() { return Err(Refusal::NotListed); }
    Ok(picked.remove(0))
}

fn destination(kind: Kind, coin: &str, stored: &Stored, e: &Entry, networks: Option<&Coin>) -> Result<Destination, Refusal> {
    let address = e.address.clone();
    match kind {
        Kind::Binance => {
            let network = e.network.clone().ok_or(Refusal::Inadmissible("the entry names no network"))?;
            let n = networks.and_then(|c| c.networks.iter().find(|n| n.network == network)).ok_or(Refusal::Inadmissible("the network is not in the coin list"))?;
            if !n.withdraw_enable { return Err(Refusal::Inadmissible("the network is closed for withdrawals")); }
            match (n.withdraw_tag, &e.tag) {
                (Some(true), None) => return Err(Refusal::Inadmissible("the network needs a tag and the entry has none")),
                (Some(false), Some(_)) => return Err(Refusal::Inadmissible("the network takes no tag and the entry has one")),
                (None, _) => return Err(Refusal::Inadmissible("the coin list does not say whether the network needs a tag")),
                _ => {}
            }
            Ok(Destination::Binance { coin: coin.to_string(), network, address, tag: e.tag.clone() })
        }
        Kind::Mexc => {
            let listed = e.network.clone().ok_or(Refusal::Inadmissible("the entry names no network"))?;
            let candidates: Vec<&Network> = networks.map(|c| c.networks.iter().filter(|n| n.network == listed || n.net_work.as_deref() == Some(listed.as_str())).collect()).unwrap_or_default();
            let mut codes: Vec<&str> = candidates.iter().filter_map(|n| n.net_work.as_deref()).collect();
            codes.sort_unstable();
            codes.dedup();
            // The stored network, by any of the listing's aliases, is checked before anything that refuses the entry.
            let on_stored = |n: Option<&Network>, code: Option<&str>| stored.network.as_ref().is_none_or(|s| *s == listed || n.is_some_and(|n| *s == n.network) || code == Some(s.as_str()));
            let [code] = codes.as_slice() else {
                // Unresolved: it is another network's entry only when none of its candidate aliases is the stored one.
                let may_be_stored = on_stored(None, None) || candidates.iter().any(|n| on_stored(Some(n), n.net_work.as_deref()));
                return Err(if may_be_stored { Refusal::Inadmissible("the entry's network maps to no single netWork") } else { Refusal::NotListed });
            };
            let n = candidates.iter().find(|n| n.net_work.as_deref() == Some(*code)).expect("a candidate names the code");
            if !on_stored(Some(n), Some(code)) { return Err(Refusal::NotListed); }
            if !n.withdraw_enable { return Err(Refusal::Inadmissible("the network is closed for withdrawals")); }
            Ok(Destination::Mexc { coin: coin.to_string(), network: n.network.clone(), net_work: code.to_string(), address, memo: e.tag.clone() })
        }
        Kind::Kraken => {
            let key = e.key.clone().ok_or(Refusal::Inadmissible("the entry names no key"))?;
            // Rails lists an entry without a method too (kraken.rb:477 compact_blank); the method only narrows the lookup.
            let method = e.network.clone().filter(|m| !crate::ruby::blank(m));
            Ok(Destination::Kraken { asset: coin.to_string(), key, address, method })
        }
        Kind::Gemini => {
            let network = gemini_network(coin).filter(|n| GEMINI_MEMO_FREE.contains(n)).ok_or(Refusal::Inadmissible("Gemini withdrawals on this network need a memo"))?;
            if e.network.as_deref().is_some_and(|n| n != network) || stored.network.as_deref().is_some_and(|n| n != network) { return Err(Refusal::NotListed); }
            Ok(Destination::Gemini { ticker: coin.to_lowercase(), network: network.to_string(), address })
        }
    }
}

// ---- fees

/// A flat coin-list fee for `network` (Binance, MEXC): unknown when the network is not listed or its fee unreadable.
pub fn network_fee(coin: &Coin, network: &str) -> Option<BigDec> {
    coin.networks.iter().find(|n| n.network == network).and_then(|n| n.fee.clone())
}

/// D-RULE-KRAKEN-QUOTE: a WithdrawInfo answer for `requested`. The envelope's error is the venue's refusal; otherwise the
/// fee is known only when both figures read, the fee is ≥ 0 and the net is exactly `requested − fee`.
pub fn kraken_quote(body: &Value, requested: &BigDec) -> Result<Option<BigDec>, VenueError> {
    super::kraken::check(body)?;
    let r = &body["result"];
    let (Some(net), Some(fee)) = (crate::ruby::json_to_d(&r["amount"]).ok().flatten(), known_fee(&r["fee"])) else { return Ok(None) };
    Ok((&net + &fee == *requested).then_some(fee))
}

/// D-RULE-GEMINI-FEE-ESTIMATE: a feeEstimate answer for `ticker`: `fee` (a decimal, ≥ 0), its `currency` the ticker.
pub fn gemini_fee(body: &Value, ticker: &str) -> Option<BigDec> {
    if !body["currency"].as_str().is_some_and(|c| c.eq_ignore_ascii_case(ticker)) { return None; }
    known_fee(&body["fee"])
}

/// What a fee read writes back (D-RULE-FEE-STAMP): the exchange_assets columns Rails keeps, for the rule's asset.
#[derive(Clone, Debug, PartialEq)]
pub struct Stamp { pub fee: String, pub chains: Option<Value> }

/// A coin list's stamp, as Rails writes it (the default network's fee; every network as {name, fee, is_default}), but
/// only when every network's fee is known.
pub fn coin_stamp(coin: &Coin) -> Option<Stamp> {
    if coin.networks.is_empty() || coin.networks.iter().any(|n| n.fee.is_none()) { return None; }
    let d = coin.default_network()?;
    let raw = |n: &Network| n.fee_raw.as_str().map_or_else(|| n.fee.as_ref().expect("known").to_s_f(), str::to_string);
    let chains = coin.networks.iter().map(|n| serde_json::json!({ "name": n.network, "fee": n.fee_raw, "is_default": n.is_default })).collect();
    Some(Stamp { fee: raw(d), chains: Some(Value::Array(chains)) })
}

/// A quote's stamp (Kraken, Gemini): the fee only.
pub fn quote_stamp(fee: &BigDec) -> Stamp { Stamp { fee: fee.to_s_f(), chains: None } }

/// Writes `stamp` to the (exchange, asset) row, stamped `at`; a missing row is not created. Returns whether one was written.
pub fn write_stamp(c: &Connection, exchange_id: i64, asset_id: i64, stamp: &Stamp, at: DateTime<Utc>) -> rusqlite::Result<bool> {
    let at = crate::codec::format_time(at);
    let chains = stamp.chains.as_ref().map(Value::to_string);
    Ok(c.execute("UPDATE exchange_assets SET withdrawal_fee = ?1, withdrawal_fee_updated_at = ?2, withdrawal_chains = COALESCE(?3, withdrawal_chains), \
                  updated_at = ?2 WHERE exchange_id = ?4 AND asset_id = ?5", params![stamp.fee, at, chains, exchange_id, asset_id])? == 1)
}

/// Exchange#symbol_from_asset: the venue code of `asset_id`, from the first available ticker naming it (base before quote).
pub fn venue_code(c: &Connection, exchange_id: i64, asset_id: i64) -> rusqlite::Result<Option<String>> {
    c.query_row("SELECT CASE WHEN base_asset_id = ?2 THEN base ELSE quote END FROM tickers WHERE exchange_id = ?1 AND available = 1 \
                 AND (base_asset_id = ?2 OR quote_asset_id = ?2) ORDER BY id LIMIT 1", params![exchange_id, asset_id], |r| r.get(0)).optional()
}

/// The key a Binance or MEXC coin list is read with (D-RULE-FEE-FALLBACK, Q1).
#[derive(Clone, Debug)]
/// `Withdrawal` carries the api_keys id.
pub enum FeeKey { Fee(crate::crypto::Credentials), Withdrawal(crate::crypto::Credentials, i64) }

/// The exchange's FeeApiKey when one is stored and readable, else the user's withdrawal key; None without either.
pub fn fee_key(c: &Connection, cipher: &crate::crypto::Cipher, user_id: i64, exchange_id: i64) -> Result<Option<FeeKey>, crate::engine::EngineError> {
    let row: Option<(Option<String>, Option<String>)> = c.query_row("SELECT key, secret FROM fee_api_keys WHERE exchange_id = ?1 ORDER BY id LIMIT 1",
                                                                    [exchange_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    if let Some((Some(key), Some(secret))) = row {
        if let (Ok(key), Ok(secret)) = (cipher.decrypt(&key), cipher.decrypt(&secret)) {
            let credentials = crate::crypto::Credentials { redaction_values: vec![key.clone(), secret.clone()], key, secret, passphrase: None, ibkr: None };
            return Ok(Some(FeeKey::Fee(credentials)));
        }
    }
    Ok(crate::engine::model::withdrawal_credentials(c, cipher, user_id, exchange_id)?.map(|(cr, v)| FeeKey::Withdrawal(cr, v.id)))
}

// ---- §6 sizing

/// `floor_p`: down to a positive `multiple` (Binance's withdrawIntegerMultiple), else to 8 decimals (D-RULE-FLOOR).
pub fn floor_p(x: &BigDec, multiple: Option<&BigDec>) -> BigDec {
    let Some(m) = multiple.filter(|m| m.is_positive()) else { return x.floor(8) };
    let Some(q) = x.div(m) else { return x.floor(8) };
    let mut out = &q.floor(0) * m;
    if out > *x { out = &out - m; }
    out
}

/// The §6 sizing's outcome.
#[derive(Clone, Debug, PartialEq)]
pub enum Sizing {
    /// Send `amount`; the fee at `budget` (`first_fee`, the trigger's) and at `amount` (`fee_cap`, Kraken's max_fee).
    Send { budget: BigDec, amount: BigDec, first_fee: BigDec, fee_cap: BigDec },
    /// A quote was unknown (D-RULE-FEE-UNKNOWN-REFUSED): Rails' "Withdrawal fee unknown" row.
    FeeUnknown { budget: BigDec },
    /// The budget does not cover the fee: Rails' "does not cover fee" row.
    DoesNotCover { budget: BigDec, fee: BigDec },
}

/// §6 steps 1, 2 and 4 with the fee charged on top (`fee_on_top` unknown: the conservative branch). `quote(x)` is the fee
/// for sending `x` (None: unknown); it is asked at the budget and again at the amount, so a percentage fee is exact.
pub async fn size<F, Fut>(free: &BigDec, pct: &BigDec, multiple: Option<&BigDec>, mut quote: F) -> Result<Sizing, VenueError>
where F: FnMut(BigDec) -> Fut, Fut: std::future::Future<Output = Result<Option<BigDec>, VenueError>> {
    size_with_semantics(free,pct,multiple,None,&mut quote).await
}

/// The fee-included branch is enabled only after the venue proves those semantics.
pub async fn size_with_semantics<F,Fut>(free:&BigDec,pct:&BigDec,multiple:Option<&BigDec>,fee_on_top:Option<bool>,mut quote:F)->Result<Sizing,VenueError>
where F:FnMut(BigDec)->Fut,Fut:std::future::Future<Output=Result<Option<BigDec>,VenueError>> {
    let hundred = BigDec::from_i64(100);
    let budget = floor_p(&(free * &pct.div(&hundred).expect("100 is not zero")), multiple);
    let Some(first_fee) = quote(budget.clone()).await?.filter(|f| *f >= BigDec::zero()) else { return Ok(Sizing::FeeUnknown { budget }) };
    if fee_on_top==Some(false) {
        if !budget.is_positive() {return Ok(Sizing::DoesNotCover{budget,fee:first_fee});}
        return Ok(Sizing::Send{amount:budget.clone(),budget,fee_cap:first_fee.clone(),first_fee});
    }
    let amount = floor_p(&(&budget - &first_fee), multiple);
    if !amount.is_positive() { return Ok(Sizing::DoesNotCover { budget, fee: first_fee }); }
    let Some(fee_cap) = quote(amount.clone()).await?.filter(|f| *f >= BigDec::zero()) else { return Ok(Sizing::FeeUnknown { budget }) };
    if &amount + &fee_cap > budget { return Ok(Sizing::DoesNotCover { budget, fee: fee_cap }); }
    Ok(Sizing::Send { budget, amount, first_fee, fee_cap })
}

// ---- E1 capital withdrawals: shared wire validation, not the trading error classifier.

pub(crate) fn capital_unknown() -> crate::engine::withdrawal::LookupResult {
    crate::engine::withdrawal::LookupResult::Unknown {
        reason: "withdrawal history is incomplete, mismatched or outside the lookup window; operator recovery via resolve-withdrawal is required".into(),
        candidates: vec![],
    }
}

/// Provably nothing was transmitted (a refusal before the request, or the transport's NotSent): settles as not sent.
pub(crate) fn capital_not_sent() -> crate::engine::withdrawal::SendResult {
    crate::engine::withdrawal::SendResult::NotSent {
        reason: "the withdrawal request is invalid or past its send window; not sent".into(),
    }
}

pub(crate) fn capital_deadline(r: &crate::engine::withdrawal::Request) -> Option<DateTime<Utc>> {
    crate::codec::parse_time(&r.created_at)
        .ok()?
        .checked_add_signed(
            chrono::Duration::seconds(crate::engine::withdrawal::SEND_WINDOW_SECONDS)
                + chrono::Duration::from_std(super::http::TOTAL_TIMEOUT).expect("45 s"),
        )
}

/// Only a clean id accepts; only a cited code and message reject. 5xx, missing codes,
/// contradictory envelopes, unreadable bodies and all uncertain transport errors retain the intent.
/// D-RULE-AMBIGUITY-ALLOWLIST and D-RULE-LOG-SECRETS: no untrusted body is logged.
pub(crate) async fn capital_send<T: super::http::Transport>(
    transport: &T,
    request: &super::http::HttpRequest,
    venue: &str,
) -> crate::engine::withdrawal::SendResult {
    use super::http::{decode_json, TransportError};
    use crate::engine::{rules, withdrawal::SendResult};
    let unknown = || SendResult::Ambiguous {
        reason: "withdrawal reply is unconfirmed".into(),
    };
    let response = match transport.send_limited(request, 64 * 1024).await {
        Ok(response) => response,
        Err(TransportError::NotSent(_)) => return capital_not_sent(),
        Err(_) => return unknown(),
    };
    if !(200..300).contains(&response.status) && !(400..500).contains(&response.status) {
        return unknown();
    }
    let Ok(body) = decode_json(&response.body) else {
        return unknown();
    };
    let Some(object) = body.as_object() else {
        return unknown();
    };
    if object.contains_key("code") {
        if ["id", "success", "result", "error", "errors"]
            .iter()
            .any(|key| object.contains_key(*key))
        {
            return unknown();
        }
        if let (Some(code), Some(message)) = (body["code"].as_i64(), body["msg"].as_str()) {
            if rules::definitive_code(venue, code, message) {
                return SendResult::Rejected {
                    definitive: true,
                    reason: rules::safe_message(message),
                };
            }
        }
        return unknown();
    }
    if !(200..300).contains(&response.status)
        || object
            .get("success")
            .is_some_and(|value| value != &Value::Bool(true))
        || ["msg", "error", "errors", "result"]
            .iter()
            .any(|key| object.contains_key(*key))
    {
        return unknown();
    }
    match text(&body["id"]) {
        Some(id) => SendResult::Accepted { id },
        None => unknown(),
    }
}

/// All rows must parse, even unrelated ones. A missing client id can be a manual withdrawal;
/// it never matches ours. The history network is optional, but a returned value must match.
/// MEXC's canonical network was resolved through networkList when the immutable destination
/// was prepared (D-RULE-LISTED-ENTRY); compare that label, never the sent netWork alias.
pub(crate) fn capital_match(
    row: &Value,
    request: &crate::engine::withdrawal::Request,
    kind: Kind,
) -> Result<Option<(String, crate::engine::withdrawal::FoundStatus)>, ()> {
    use crate::engine::withdrawal::FoundStatus;
    let required = |key: &str| {
        row[key]
            .as_str()
            .filter(|s| !crate::ruby::blank(s))
            .ok_or(())
    };
    let optional = |key: &str| match row.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        _ => Err(()),
    };
    let id = required("id")?;
    let coin = required("coin")?;
    let address = required("address")?;
    let amount = crate::ruby::json_to_d(&row["amount"])
        .ok()
        .flatten()
        .filter(BigDec::is_positive)
        .ok_or(())?;
    let status = row["status"].as_i64().ok_or(())?;
    let client_id = optional("withdrawOrderId")?;
    let network = optional("network")?;
    let tag = optional(if kind == Kind::Mexc {
        "memo"
    } else {
        "addressTag"
    })?;
    let info = optional("info")?;
    let (expected_coin, expected_address, expected_network, expected_tag) =
        match (&request.destination, kind) {
            (
                Destination::Binance {
                    coin,
                    address,
                    network,
                    tag,
                },
                Kind::Binance,
            ) => {
                let wallet = row["walletType"]
                    .as_i64()
                    .filter(|w| matches!(w, 0 | 1))
                    .ok_or(())?;
                if client_id == Some(request.client_id.as_str())
                    && (wallet != 0 || request.wallet_type != Some(0))
                {
                    return Err(());
                }
                (coin, address, network, tag)
            }
            (
                Destination::Mexc {
                    coin,
                    address,
                    network,
                    memo,
                    ..
                },
                Kind::Mexc,
            ) => (coin, address, network, memo),
            _ => return Err(()),
        };
    if client_id != Some(request.client_id.as_str()) {
        return Ok(None);
    }
    if coin != expected_coin
        || coin != request.coin
        || address != expected_address
        || amount != BigDec::parse(&request.amount).map_err(|_| ())?
        || network.is_some_and(|n| n != expected_network)
        || tag.is_some_and(|t| t != expected_tag.as_deref().unwrap_or(""))
    {
        return Err(());
    }
    let outcome = match (kind, status) {
        (Kind::Binance, 0 | 2 | 4 | 6) | (Kind::Mexc, 1..=7 | 10) => FoundStatus::Sent,
        (Kind::Binance, 1 | 3 | 5) | (Kind::Mexc, 8 | 9) => FoundStatus::Failed {
            reason: crate::engine::rules::safe_message(
                info.filter(|s| !crate::ruby::blank(s))
                    .unwrap_or("the venue reports that the withdrawal failed"),
            ),
        },
        _ => return Err(()),
    };
    Ok(Some((id.into(), outcome)))
}
