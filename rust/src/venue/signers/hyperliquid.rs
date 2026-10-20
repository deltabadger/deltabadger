//! hyperliquid-rb 0.1.1's L1 signing, as honeymaker 0.12.5's wrapper drives it (`build_exchange`): one order
//! (`Exchange#order` → `bulk_orders`, grouping "na") or one cancel (`Exchange#cancel` → `bulk_cancel`), mainnet, no vault and
//! no `expires_after`, the nonce from one process-wide monotonic source shared by both (`NONCE`).
//! - `Signer#action_hash` = keccak256(MessagePack.pack(action) ‖ nonce u64 BE ‖ 0x00);
//! - the phantom agent `{source: "a", connectionId: hash}` hashed as EIP-712 (`Agent(string source,bytes32 connectionId)`,
//!   domain Exchange / 1 / chainId 1337 / the zero address) and signed by `Eth::Key#sign` (secp256k1, RFC 6979, low s);
//! - `{r, s, v}` as `sign_typed_data` splits it: r and s as `0x` + hex without leading zeros, v = 27 + recovery id.
use crate::ruby::BigDec;
use k256::ecdsa::SigningKey;
use serde_json::{json, Value};
use sha3::{Digest, Keccak256};

/// Honeymaker::Clients::Hyperliquid.next_nonce: one per process, for orders and cancels alike.
pub static NONCE: super::Nonce = super::Nonce::new();

/// `Utils.order_request_to_order_wire` for a limit order: `p`/`s` already through `float_to_wire`; `c` only with a cloid.
pub struct OrderWire { pub asset: u64, pub is_buy: bool, pub price: String, pub size: String, pub reduce_only: bool, pub tif: &'static str, pub cloid: Option<String> }

pub enum Action { Order(OrderWire), Cancel { asset: u64, oid: u64 } }

#[derive(Debug, PartialEq, Eq)]
pub struct Signature { pub r: String, pub s: String, pub v: u8 }

/// `Utils.float_to_wire` over the BigDecimal Rails passes: `format("%.8f", x)` (through the double), refused when that
/// double, coerced back to a BigDecimal (Float#to_d: 16 significant digits), is 1e-12 or more away from x; then trailing
/// zeros (and a bare `.`) dropped. A refusal is a refused order, never re-rounded (PLAN-VB §1.5).
pub fn float_to_wire(x: &BigDec) -> Result<String, String> {
    let refused = || format!("float_to_wire causes rounding: {}", x.to_s_f());
    let rounded = format!("{:.8}", x.to_f());
    let back = rounded.parse::<f64>().ok().and_then(|f| BigDec::from_f64(f).ok()).ok_or_else(refused)?;
    let gap = &back - x;
    let tolerance = BigDec::parse("1e-12").expect("a literal");
    if gap >= tolerance || gap <= &BigDec::zero() - &tolerance { return Err(refused()); }
    Ok(rounded.trim_end_matches('0').trim_end_matches('.').to_string())
}

impl Action {
    /// The action as posted (and as packed: the same keys in the same order).
    pub fn to_json(&self) -> Value {
        match self {
            Action::Order(o) => {
                let mut w = json!({ "a": o.asset, "b": o.is_buy, "p": o.price, "s": o.size, "r": o.reduce_only, "t": { "limit": { "tif": o.tif } } });
                if let Some(c) = &o.cloid { w["c"] = json!(c); }
                json!({ "type": "order", "orders": [w], "grouping": "na" })
            }
            Action::Cancel { asset, oid } => json!({ "type": "cancel", "cancels": [{ "a": asset, "o": oid }] }),
        }
    }

    /// MessagePack.pack(action) for these shapes: string keys in order, the smallest unsigned int, true/false, nested maps
    /// and arrays. Nothing else occurs in an order or a cancel.
    pub fn msgpack(&self) -> Vec<u8> {
        let mut out = Vec::new();
        pack(&self.to_json(), &mut out);
        out
    }
}

fn pack(v: &Value, out: &mut Vec<u8>) {
    let len = |out: &mut Vec<u8>, n: usize, fix: u8, fix_max: usize, wide: [u8; 2]| {
        if n <= fix_max { out.push(fix | n as u8) } else if n <= 0xffff { out.push(wide[0]); out.extend((n as u16).to_be_bytes()) } else { out.push(wide[1]); out.extend((n as u32).to_be_bytes()) }
    };
    match v {
        Value::Bool(b) => out.push(if *b { 0xc3 } else { 0xc2 }),
        Value::Number(n) => {
            let n = n.as_u64().expect("only unsigned ints are packed");
            match n {
                0..=0x7f => out.push(n as u8),
                0x80..=0xff => out.extend([0xcc, n as u8]),
                0x100..=0xffff => { out.push(0xcd); out.extend((n as u16).to_be_bytes()) }
                0x1_0000..=0xffff_ffff => { out.push(0xce); out.extend((n as u32).to_be_bytes()) }
                _ => { out.push(0xcf); out.extend(n.to_be_bytes()) }
            }
        }
        Value::String(s) => {
            let n = s.len();
            if n <= 31 { out.push(0xa0 | n as u8) } else if n <= 0xff { out.extend([0xd9, n as u8]) } else { len(out, n, 0, 0, [0xda, 0xdb]) }
            out.extend(s.as_bytes());
        }
        Value::Array(a) => { len(out, a.len(), 0x90, 15, [0xdc, 0xdd]); a.iter().for_each(|x| pack(x, out)) }
        Value::Object(m) => { len(out, m.len(), 0x80, 15, [0xde, 0xdf]); for (k, x) in m { pack(&Value::from(k.as_str()), out); pack(x, out) } }
        Value::Null => out.push(0xc0),
    }
}

fn keccak(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Keccak256::new();
    parts.iter().for_each(|p| h.update(p));
    h.finalize().into()
}

/// `Signer#action_hash` with no vault and no expiry.
pub fn action_hash(action: &Action, nonce: u64) -> [u8; 32] { keccak(&[&action.msgpack(), &nonce.to_be_bytes(), &[0]]) }

/// `Eth::Eip712.hash` of the phantom agent: keccak(0x1901 ‖ domain separator ‖ hashStruct(Agent)).
fn agent_digest(connection_id: &[u8; 32]) -> [u8; 32] {
    let mut chain_id = [0u8; 32];
    chain_id[30..].copy_from_slice(&1337u16.to_be_bytes());
    let domain = keccak(&[&keccak(&[b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"]),
                          &keccak(&[b"Exchange"]), &keccak(&[b"1"]), &chain_id, &[0u8; 32]]);
    let agent = keccak(&[&keccak(&[b"Agent(string source,bytes32 connectionId)"]), &keccak(&[b"a"]), connection_id]);
    keccak(&[b"\x19\x01", &domain, &agent])
}

/// The agent key as hyperliquid-rb reads it: hex, with or without `0x`.
pub fn signing_key(secret: &str) -> Option<SigningKey> {
    let bytes = hex::decode(secret.trim().strip_prefix("0x").unwrap_or(secret.trim())).ok()?;
    SigningKey::from_slice(&bytes).ok()
}

/// `Signer#sign_l1_action(action, nonce:)`.
pub fn sign_l1_action(key: &SigningKey, action: &Action, nonce: u64) -> Signature {
    let (sig, recovery) = key.sign_prehash_recoverable(&agent_digest(&action_hash(action, nonce))).expect("a 32-byte digest signs");
    let (r, s) = sig.split_bytes();
    let hex = |b: &[u8]| { let h = hex::encode(b); format!("0x{}", match h.trim_start_matches('0') { "" => "0", t => t }) };
    Signature { r: hex(&r), s: hex(&s), v: 27 + recovery.to_byte() }
}

/// The `/exchange` body `post_action` sends: `{action, nonce, signature}`.
pub fn exchange_body(action: &Action, nonce: u64, signature: &Signature) -> String {
    json!({ "action": action.to_json(), "nonce": nonce, "signature": { "r": signature.r, "s": signature.s, "v": signature.v } }).to_string()
}
