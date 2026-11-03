//! Each crypto venue's public catalogue, as `Exchanges::<Venue>#get_tickers_info(force: true)` reads it (PLAN-A17 A17-2):
//! one row per pair, the hash Rails builds — `ticker, base, quote, minimum_base_size, minimum_quote_size,
//! maximum_base_size, maximum_quote_size, base_decimals, quote_decimals, price_decimals, available, trading_enabled` —
//! with Rails' JSON types (`as_json`): a `.to_d` size is BigDecimal#to_s's plain string ("5.0"), an Integer a number,
//! nil null, and a passed-through venue value as the venue sent it. That is the data-api ticker row shape
//! `jobs::import::ticker_records_for` reads (sizes as strings or numbers), less the `*_external_id`s the synchronizer adds.
//! Public endpoints only: no key, nothing signed. Request contracts are Rails' (honeymaker 0.12.5's clients), pinned by
//! rust/tests/catalogue.rs against script/rust/catalogue_vectors.rb's recordings.
use super::http::{self, HttpRequest, Transport, TransportError};
use crate::ruby::{self, BigDec};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::time::Duration;

pub const BYBIT_INSTRUMENTS: &str = "/v5/market/instruments-info";
pub const BITGET_SYMBOLS: &str = "/api/v2/spot/public/symbols";
pub const GEMINI_SYMBOLS: &str = "/v1/symbols";
pub const GEMINI_DETAILS: &str = "/v1/symbols/details/";
pub const KRAKEN_ASSET_PAIRS: &str = "/0/public/AssetPairs";
pub const KRAKEN: &str = "Exchanges::Kraken";
/// A catalogue answer (Binance's exchangeInfo is the largest, a few MB).
const MAX_BODY: usize = http::MAX_BODY;
/// One Gemini symbol's details.
const DETAIL_MAX_BODY: usize = 64 * 1024;
/// Exchanges::Gemini::CATALOGUE_REQUEST_INTERVAL, slept before each detail request.
pub const GEMINI_INTERVAL: Duration = Duration::from_secs(1);
/// Exchange#with_transient_retry(base_delay: 2) as Gemini's catalogue calls it: 3 attempts, waits of 2 s and 4 s.
const GEMINI_ATTEMPTS: u64 = 3;
const GEMINI_BASE_DELAY: u64 = 2;
/// Exchanges::Coinbase::ASSET_BLACKLIST.
const COINBASE_BLACKLIST: [&str; 3] = ["RNDR", "ZETACHAIN", "WAXL"];
/// Exchanges::Kraken::REAL_COSTMIN.
const KRAKEN_REAL_COSTMIN: [(&str, &str); 16] = [("AUD", "10"), ("CAD", "5"), ("CHF", "5"), ("DAI", "5"), ("ETH", "0.002"), ("EUR", "0.5"),
    ("GBP", "5"), ("JPY", "500"), ("PYUSD", "5"), ("RLUSD", "5"), ("USD", "5"), ("USDC", "5"), ("USDQ", "5"), ("USDR", "5"), ("USDT", "5"),
    ("XBT", "0.00005")];

/// The venue's catalogue rows, or the failure Rails' `get_tickers_info` returns (its errors as `to_sentence`).
/// `venue_type` is the STI type ("Exchanges::Kraken"); a stock venue, a retired one (Bitmart) or an unknown type is an error.
pub async fn read<T: Transport>(venue_type: &str, t: &T) -> Result<Vec<Value>, String> {
    use super::{binance, bingx, bitget, bitrue, bitvavo, bybit, coinbase, gemini, hyperliquid, kucoin, mexc};
    use crate::sync::{binance_ledger, bingx_ledger, bitrue_ledger, kucoin_hf_ledger, mexc_ledger};
    let row = super::registry::ROWS.iter().find(|r| r.exchange_type == venue_type).ok_or_else(|| format!("{venue_type}: no catalogue"))?;
    let (base, name) = (row.base, row.name);
    let get = |path: &str, query: Vec<(&'static str, String)>| ask(t, "GET", base, path.to_string(), query, None, MAX_BODY);
    match venue_type {
        binance::BINANCE | binance::BINANCE_US => {
            parse_binance(&get(binance_ledger::EXCHANGE_INFO_PATH, vec![("permissions", r#"["SPOT"]"#.into())]).await.map_err(|f| f.venue_message(&["msg"]))?)
        }
        bingx::BINGX => parse_bingx(&get(bingx_ledger::SYMBOLS_PATH, vec![]).await.map_err(|f| f.venue_message(&["msg"]))?),
        bitrue::BITRUE => parse_bitrue(&get(bitrue_ledger::EXCHANGE_INFO_PATH, vec![]).await.map_err(|f| f.venue_message(&["msg"]))?),
        mexc::MEXC => parse_mexc(&get(mexc_ledger::EXCHANGE_INFO_PATH, vec![]).await.map_err(|f| f.venue_message(&["msg"]))?),
        bybit::BYBIT => parse_bybit(&get(BYBIT_INSTRUMENTS, vec![("category", "spot".into())]).await.map_err(|f| f.venue_message(&["retMsg", "msg"]))?),
        bitvavo::BITVAVO => parse_bitvavo(&get(bitvavo::MARKETS_PATH, vec![]).await.map_err(Failure::sentence)?),
        kucoin::KUCOIN => parse_kucoin(name, &get(kucoin_hf_ledger::SYMBOLS_PATH, vec![]).await.map_err(Failure::sentence)?),
        KRAKEN => parse_kraken(&get(KRAKEN_ASSET_PAIRS, vec![("aclass_base", "all".into())]).await.map_err(Failure::sentence)?),
        coinbase::COINBASE => parse_coinbase(&get(coinbase::PRODUCTS, vec![]).await.map_err(|_| format!("Failed to get {name} products"))?),
        hyperliquid::HYPERLIQUID => {
            let body = Some(json!({ "type": "spotMeta" }));
            parse_hyperliquid(&ask(t, "POST", base, hyperliquid::INFO.into(), vec![], body, MAX_BODY).await.map_err(Failure::sentence)?)
        }
        bitget::BITGET => {
            let symbols = get(BITGET_SYMBOLS, vec![]).await.map_err(Failure::sentence)?;
            if symbols["data"].is_null() { return Err(format!("Failed to get {name} symbols")); }
            // Exchanges::Bitget#get_tickers_prices: a failed price read leaves the minimums at their fallback.
            let tickers = get(bitget::TICKERS_PATH, vec![]).await.ok();
            parse_bitget(name, &symbols, tickers.as_ref())
        }
        gemini::GEMINI => read_gemini(t, base, name).await,
        _ => Err(format!("{venue_type}: no catalogue")),
    }
}

/// Gemini: the symbol list, then each symbol's details, 1 s apart, each read retried as Rails retries it; any failure fails
/// the whole catalogue (all or nothing: the sync marks every ticker missing from it unavailable).
async fn read_gemini<T: Transport>(t: &T, base: &str, name: &str) -> Result<Vec<Value>, String> {
    let symbols = gemini_ask(t, base, GEMINI_SYMBOLS.into(), MAX_BODY).await.map_err(Failure::sentence)?;
    if symbols.is_null() { return Err(format!("Failed to get {name} symbols")); }
    let mut rows = vec![];
    for symbol in symbols.as_array().ok_or("undefined method 'filter_map' for the symbol list")? {
        tokio::time::sleep(GEMINI_INTERVAL).await;
        let symbol = to_s(symbol);
        let detail = gemini_ask(t, base, format!("{GEMINI_DETAILS}{symbol}"), DETAIL_MAX_BODY).await
            .map_err(|f| format!("{name} {symbol}: {}", f.sentence()))?;
        rows.extend(gemini_row(name, &symbol, &detail)?);
    }
    gemini_twins(name, rows)
}

/// with_transient_retry(base_delay: 2, retry_if: catalogue_retry?): a 408, 429 or 5xx answer, or a transport failure that
/// may pass, is asked again after 2 s, then 4 s.
async fn gemini_ask<T: Transport>(t: &T, base: &str, path: String, limit: usize) -> Result<Value, Failure> {
    let mut tries = 0;
    loop {
        tries += 1;
        match ask(t, "GET", base, path.clone(), vec![], None, limit).await {
            Err(f) if tries < GEMINI_ATTEMPTS && f.retryable() => tokio::time::sleep(Duration::from_secs(GEMINI_BASE_DELAY * tries)).await,
            other => return other,
        }
    }
}

/// A failed request as Honeymaker's Client#with_rescue returns it: the answer's body (or the transport's message) and its
/// status (None: no answer).
#[derive(Debug)]
struct Failure { errors: Vec<String>, status: Option<u16>, transient: bool }

impl Failure {
    fn sentence(self) -> String { ruby::to_sentence(&self.errors) }
    /// parse_error_message: the body's `keys[0]` (Bybit: `retMsg || msg`) when the body is a JSON object naming one, else
    /// the failure as it came.
    fn venue_message(self, keys: &[&str]) -> String {
        let named = self.errors.first().and_then(|e| serde_json::from_str::<Value>(e).ok())
            .and_then(|body| keys.iter().map(|k| body[*k].clone()).find(truthy))
            .filter(present);
        match named { Some(m) => to_s(&m), None => self.sentence() }
    }
    /// Exchanges::Gemini#catalogue_retry?.
    fn retryable(&self) -> bool { self.status.map_or(self.transient, |s| s == 408 || s == 429 || s >= 500) }
}

async fn ask<T: Transport>(t: &T, method: &'static str, base: &str, path: String, query: Vec<(&'static str, String)>, body: Option<Value>, limit: usize)
                           -> Result<Value, Failure> {
    let req = HttpRequest { method, base: base.into(), path, query, body, not_after: None };
    let resp = match t.send_limited(&req, limit).await {
        Ok(resp) => resp,
        Err(e) => {
            let (m, transient) = match e { TransportError::NotSent(m) | TransportError::MaybeSent(m) => (m, true), TransportError::Permanent(m) => (m, false) };
            return Err(Failure { errors: vec![m], status: None, transient });
        }
    };
    // Faraday's raise_error: the body is the error, or the status line when there is none.
    let as_error = || if resp.body.is_empty() { format!("the server responded with status {}", resp.status) } else { resp.body.clone() };
    if resp.status >= 400 { return Err(Failure { errors: vec![as_error()], status: Some(resp.status), transient: false }); }
    if resp.body.trim().is_empty() { return Ok(Value::Null); }
    http::decode_json(&resp.body).map_err(|_| Failure { errors: vec![as_error()], status: Some(resp.status), transient: false })
}

// --- the parsers: Rails' field rules over an answer, one to one --------------------------------------------------------

pub fn parse_binance(info: &Value) -> Result<Vec<Value>, String> {
    list(&info["symbols"])?.iter().map(|p| {
        let filters = dig(p, &["filters"])?;
        let (price, lot, notional) = (filter(filters, &["PRICE_FILTER"])?, filter(filters, &["LOT_SIZE"])?, filter(filters, &["NOTIONAL", "MIN_NOTIONAL"])?);
        let (price, lot, notional) = (some(price)?, some(lot)?, some(notional)?);
        Ok(row([dig(p, &["symbol"])?.clone(), dig(p, &["baseAsset"])?.clone(), dig(p, &["quoteAsset"])?.clone(),
                d(&lot["minQty"])?, d(&notional["minNotional"])?, d(&lot["maxQty"])?, d(&notional["maxNotional"])?,
                json!(decimals(&lot["stepSize"])), dig(p, &["quoteAssetPrecision"])?.clone(), json!(decimals(&price["tickSize"])),
                json!(*dig(p, &["status"])? == "TRADING")]))
    }).collect()
}

pub fn parse_bingx(answer: &Value) -> Result<Vec<Value>, String> {
    if to_i(&answer["code"])? != 0 { return Err(to_s(&answer["msg"])); }
    let mut rows = vec![];
    for p in list(dig(answer, &["data", "symbols"])?)? {
        let ticker = dig(p, &["symbol"])?;
        let parts = split(string(ticker)?, '-');
        let [base, quote] = parts[..] else { continue };
        rows.push(row([ticker.clone(), json!(base), json!(quote), json!(0), d(&p["minNotional"])?, Value::Null, d(&p["maxNotional"])?,
                       json!(decimals(&p["stepSize"])), json!(decimals(&p["tickSize"])), json!(decimals(&p["tickSize"])),
                       json!(to_i(&p["status"])? == 1)]));
    }
    Ok(rows)
}

/// `tickers`: the market tickers answer, None when that request failed (every non-USDT minimum then falls back).
pub fn parse_bitget(name: &str, symbols: &Value, tickers: Option<&Value>) -> Result<Vec<Value>, String> {
    if symbols["data"].is_null() { return Err(format!("Failed to get {name} symbols")); }
    let mut prices: HashMap<String, BigDec> = HashMap::new();
    if let Some(data) = tickers.map(|t| &t["data"]).filter(|d| !d.is_null()) {
        for t in list(data)? { prices.insert(to_s(dig(t, &["symbol"])?), to_d(dig(t, &["lastPr"])?)?); }
    }
    list(&symbols["data"])?.iter().map(|p| {
        let quote = dig(p, &["quoteCoin"])?;
        let usdt_minimum = match &p["minTradeUSDT"] { Value::Null => None, v => Some(to_d(v)?) }; // None: Integer 0
        // Exchanges::Bitget#quote_minimum.
        let q = to_s(quote);
        let minimum_quote = match usdt_minimum {
            None => json!(0),
            Some(u) if q == "USDT" || u.is_zero() => dec(&u),
            Some(u) => {
                let price = |k: String| prices.get(&k).cloned().unwrap_or_else(BigDec::zero);
                let (direct, inverse) = (price(format!("{q}USDT")), price(format!("USDT{q}")));
                if direct.is_positive() { dec(&u.div(&direct).expect("a positive divisor")) }
                else if inverse.is_positive() { dec(&(&u * &inverse)) } else { json!(0) }
            }
        };
        let maximum_base = match &p["maxTradeAmount"] { Value::Null => Value::Null, v => d(v)? };
        Ok(row([dig(p, &["symbol"])?.clone(), dig(p, &["baseCoin"])?.clone(), quote.clone(), d(dig(p, &["minTradeAmount"])?)?, minimum_quote,
                maximum_base, Value::Null, json!(to_i(dig(p, &["quantityPrecision"])?)?), json!(to_i(dig(p, &["quotePrecision"])?)?),
                json!(to_i(dig(p, &["pricePrecision"])?)?), json!(*dig(p, &["status"])? == "online")]))
    }).collect()
}

pub fn parse_bitrue(info: &Value) -> Result<Vec<Value>, String> {
    list(&info["symbols"])?.iter().map(|p| {
        let none = vec![];
        let filters = if truthy(&p["filters"]) { list(&p["filters"])? } else { &none };
        let (price, lot) = (filter_in(filters, &["PRICE_FILTER"])?, filter_in(filters, &["LOT_SIZE"])?);
        let field = |f: Option<&Value>, k: &str| f.map_or(Value::Null, |f| f[k].clone());
        Ok(row([dig(p, &["symbol"])?.clone(), upcase(&p["baseAsset"])?, upcase(&p["quoteAsset"])?,
                d(&field(lot, "minQty"))?, d(&field(lot, "minVal"))?, d(&field(lot, "maxQty"))?, json!("0.0"),
                match lot { Some(l) => json!(decimals(&l["stepSize"])), None => dig(p, &["baseAssetPrecision"])?.clone() },
                dig(p, &["quotePrecision"])?.clone(),
                match price { Some(f) => json!(decimals(&f["tickSize"])), None => dig(p, &["quotePrecision"])?.clone() },
                json!(*dig(p, &["status"])? == "TRADING")]))
    }).collect()
}

pub fn parse_bitvavo(markets: &Value) -> Result<Vec<Value>, String> {
    list(markets)?.iter().map(|p| {
        let market = dig(p, &["market"])?;
        let parts = split(string(market)?, '-');
        let part = |i: usize| parts.get(i).map_or(Value::Null, |s| json!(s));
        Ok(row([market.clone(), part(0), part(1), d(&p["minOrderInBaseAsset"])?, d(&p["minOrderInQuoteAsset"])?, Value::Null, Value::Null,
                or(&p["quantityDecimals"], &json!(8)).clone(), or(&p["notionalDecimals"], &json!(8)).clone(),
                json!(decimals(dig(p, &["tickSize"])?)), json!(*dig(p, &["status"])? == "trading")]))
    }).collect()
}

pub fn parse_bybit(answer: &Value) -> Result<Vec<Value>, String> {
    if !matches!(&answer["retCode"], Value::Number(n) if n.as_f64() == Some(0.0)) { return Err(to_s(&answer["retMsg"])); }
    list(dig(answer, &["result", "list"])?)?.iter().map(|p| {
        let (lot, price) = (dig(p, &["lotSizeFilter"])?, dig(p, &["priceFilter"])?);
        Ok(row([dig(p, &["symbol"])?.clone(), dig(p, &["baseCoin"])?.clone(), dig(p, &["quoteCoin"])?.clone(),
                d(&lot["minOrderQty"])?, d(&lot["minOrderAmt"])?, d(&lot["maxOrderQty"])?, d(&lot["maxOrderAmt"])?,
                json!(decimals(&lot["basePrecision"])), json!(decimals(&lot["quotePrecision"])), json!(decimals(&price["tickSize"])),
                json!(*dig(p, &["status"])? == "Trading")]))
    }).collect()
}

pub fn parse_coinbase(answer: &Value) -> Result<Vec<Value>, String> {
    let mut rows = vec![];
    for p in list(&answer["products"])? {
        let ticker = dig(p, &["product_id"])?;
        let parts = split(string(ticker)?, '-');
        let part = |i: usize| parts.get(i).map_or(Value::Null, |s| json!(s));
        if parts.first().is_some_and(|b| COINBASE_BLACKLIST.contains(b)) { continue; }
        // Coinbase only signals problems explicitly; absent flags are enabled. limit_only refuses the bots' market orders.
        let flagged = ["trading_disabled", "limit_only", "cancel_only", "is_disabled"].iter().any(|k| p[*k] == Value::Bool(true))
            || p.get("status").is_some_and(|s| s != "online");
        rows.push(row([ticker.clone(), part(0), part(1), d(dig(p, &["base_min_size"])?)?, d(dig(p, &["quote_min_size"])?)?,
                       d(dig(p, &["base_max_size"])?)?, d(dig(p, &["quote_max_size"])?)?, json!(decimals(dig(p, &["base_increment"])?)),
                       json!(decimals(dig(p, &["quote_increment"])?)), json!(decimals(dig(p, &["price_increment"])?)), json!(!flagged)]));
    }
    Ok(rows)
}

/// `details`: each listed symbol with its details answer, in list order.
pub fn parse_gemini(name: &str, details: &[(String, Value)]) -> Result<Vec<Value>, String> {
    let mut rows = vec![];
    for (symbol, detail) in details { rows.extend(gemini_row(name, symbol, detail)?); }
    gemini_twins(name, rows)
}

/// One symbol's row: None for a non-spot product (Gemini lists its perpetuals beside spot, under the same base and quote).
fn gemini_row(name: &str, symbol: &str, detail: &Value) -> Result<Option<Value>, String> {
    if !detail.is_object() { return Err(format!("{name} {symbol}: undefined method '[]' for its details")); }
    match &detail["product_type"] {
        Value::String(t) if t == "spot" => {}
        Value::String(_) => return Ok(None),
        _ => return Err(format!("{name} {symbol}: no product_type in its details")),
    }
    // `x&.to_d || '0.01'.to_d`, counted on BigDecimal#to_s, which ActiveSupport writes in plain notation.
    let step = |k: &str| -> Result<i64, String> {
        let v = if detail[k].is_null() { BigDec::parse("0.01").expect("a decimal") } else { to_d(&detail[k])? };
        Ok(decimals_text(&v.to_s_f()))
    };
    let quote_step = step("quote_increment")?;
    Ok(Some(row([json!(symbol), upcase(dig(detail, &["base_currency"])?)?, upcase(dig(detail, &["quote_currency"])?)?,
                 d(dig(detail, &["min_order_size"])?)?, json!(0), Value::Null, Value::Null, json!(step("tick_size")?), json!(quote_step),
                 json!(quote_step), json!(*dig(detail, &["status"])? == "open")])))
}

/// Two symbols for one pair would both be written to the same row: the list is refused.
fn gemini_twins(name: &str, rows: Vec<Value>) -> Result<Vec<Value>, String> {
    let mut groups: Vec<((&Value, &Value), Vec<&Value>)> = vec![];
    for r in &rows {
        let key = (&r["base"], &r["quote"]);
        match groups.iter_mut().find(|(k, _)| *k == key) { Some((_, g)) => g.push(&r["ticker"]), None => groups.push((key, vec![&r["ticker"]])) }
    }
    if let Some(((base, quote), twins)) = groups.iter().find(|(_, g)| g.len() > 1) {
        let names: Vec<String> = twins.iter().map(|t| to_s(t)).collect();
        return Err(format!("{name} {}/{} is listed as both {}", to_s(base), to_s(quote), names.join(" and ")));
    }
    Ok(rows)
}

pub fn parse_hyperliquid(meta: &Value) -> Result<Vec<Value>, String> {
    let tokens = list(&meta["tokens"])?;
    // tokens.to_h { |t| [t['index'], t] }: the last token of an index wins.
    let token = |index: &Value| tokens.iter().rev().find(|t| t["index"] == *index);
    let mut rows = vec![];
    for pair in list(&meta["universe"])? {
        let ids = pair["tokens"].as_array().ok_or("undefined method '[]' for a pair's tokens")?;
        let (Some(base), Some(quote)) = (token(ids.first().unwrap_or(&Value::Null)), token(ids.get(1).unwrap_or(&Value::Null))) else { continue };
        // A fixed 10 USDC minimum (smaller spot orders are rejected), 2 USDC decimals, 5 significant price figures.
        rows.push(row([pair["name"].clone(), base["name"].clone(), quote["name"].clone(), json!(0), json!(10), Value::Null, Value::Null,
                       or(&base["szDecimals"], &json!(0)).clone(), json!(2), json!(5), json!(true)]));
    }
    Ok(rows)
}

pub fn parse_kraken(answer: &Value) -> Result<Vec<Value>, String> {
    let errors = dig(answer, &["error"])?.as_array().ok_or("undefined method 'any?' for the error")?;
    if errors.iter().any(truthy) { return Err(ruby::to_sentence(&errors.iter().map(to_s).collect::<Vec<_>>())); }
    // Every tokenized pair comes back twice (an SPV key and an x key) under one wsname: keyed by wsname, the first wins.
    let mut pairs: Vec<(&str, &Value)> = vec![];
    for info in answer["result"].as_object().ok_or("undefined method 'each_with_object' for the result")?.values() {
        let ws = &info["wsname"];
        if !present(ws) { continue; }
        let ws = string(ws)?;
        if !pairs.iter().any(|(w, _)| *w == ws) { pairs.push((ws, info)); }
    }
    pairs.into_iter().map(|(ws, info)| {
        let parts = split(ws, '/');
        let part = |i: usize| parts.get(i).map_or(Value::Null, |s| json!(s));
        let real = parts.get(1).and_then(|q| KRAKEN_REAL_COSTMIN.iter().find(|(c, _)| c == q)).map(|(_, m)| BigDec::parse(m).expect("a decimal"));
        let minimum_quote = match real { Some(m) => dec(&m), None => d(or(&info["costmin"], &json!(0)))? };
        // Tokenized equities are listed but never tradable from here (AddOrder lacks their asset_class).
        let enabled = info["aclass_base"] != "tokenized_asset" && info.get("status").is_none_or(|s| s == "online");
        Ok(row([dig(info, &["altname"])?.clone(), part(0), part(1), d(dig(info, &["ordermin"])?)?, minimum_quote, Value::Null, Value::Null,
                dig(info, &["lot_decimals"])?.clone(), dig(info, &["cost_decimals"])?.clone(), dig(info, &["pair_decimals"])?.clone(), json!(enabled)]))
    }).collect()
}

pub fn parse_kucoin(name: &str, answer: &Value) -> Result<Vec<Value>, String> {
    if answer["data"].is_null() { return Err(format!("Failed to get {name} symbols")); }
    list(&answer["data"])?.iter().map(|p| {
        Ok(row([dig(p, &["symbol"])?.clone(), dig(p, &["baseCurrency"])?.clone(), dig(p, &["quoteCurrency"])?.clone(),
                d(dig(p, &["baseMinSize"])?)?, d(dig(p, &["quoteMinSize"])?)?, d(dig(p, &["baseMaxSize"])?)?, d(dig(p, &["quoteMaxSize"])?)?,
                json!(decimals(dig(p, &["baseIncrement"])?)), json!(decimals(dig(p, &["quoteIncrement"])?)),
                json!(decimals(dig(p, &["priceIncrement"])?)), dig(p, &["enableTrading"])?.clone()]))
    }).collect()
}

pub fn parse_mexc(info: &Value) -> Result<Vec<Value>, String> {
    list(&info["symbols"])?.iter().map(|p| {
        let none = vec![];
        let filters = if truthy(&p["filters"]) { list(&p["filters"])? } else { &none };
        let (price, lot, notional) = (filter_in(filters, &["PRICE_FILTER"])?, filter_in(filters, &["LOT_SIZE"])?, filter_in(filters, &["NOTIONAL", "MIN_NOTIONAL"])?);
        let field = |f: Option<&Value>, k: &str| f.map_or(Value::Null, |f| f[k].clone());
        // MEXC sends no LOT_SIZE or NOTIONAL filter: baseSizePrecision is the minimum quantity, the larger of
        // quoteAmountPrecision and quoteAmountPrecisionMarket the minimum amount, maxQuoteAmount the cap.
        let minimum_quote = match field(notional, "minNotional") {
            Value::Null => dec(&to_d(&p["quoteAmountPrecision"])?.max(to_d(&p["quoteAmountPrecisionMarket"])?)),
            v => d(&v)?,
        };
        Ok(row([dig(p, &["symbol"])?.clone(), dig(p, &["baseAsset"])?.clone(), dig(p, &["quoteAsset"])?.clone(),
                d(or(&field(lot, "minQty"), &p["baseSizePrecision"]))?, minimum_quote, d(&field(lot, "maxQty"))?,
                d(or(&field(notional, "maxNotional"), &p["maxQuoteAmount"]))?,
                match lot { Some(l) => json!(decimals(&l["stepSize"])), None => dig(p, &["baseAssetPrecision"])?.clone() },
                dig(p, &["quoteAssetPrecision"])?.clone(),
                match price { Some(f) => json!(decimals(&f["tickSize"])), None => dig(p, &["quotePrecision"])?.clone() },
                // status is "1" even for symbols MEXC refuses spot orders on: isSpotTradingAllowed is the gate.
                json!(*dig(p, &["status"])? == "1" && p["isSpotTradingAllowed"] != Value::Bool(false))]))
    }).collect()
}

// --- decimals ----------------------------------------------------------------------------------------------------------

/// `Utilities::Number.decimals(value)`: the digits after the point of `value.to_s`, trailing zeros stripped (a JSON
/// string as sent, a Float as Float#to_s writes it, an Integer 0, nil 0).
///
/// **D-CG-DECIMALS-EXPONENT** (PLAN-A17): Rails counts the characters after the point of whatever `to_s` wrote, so a value
/// in exponent form reads wrong — a JSON number below 1e-4 is a Float that Float#to_s writes as "1.0e-06" (BingX sends
/// its steps as numbers: Rails says 5, so orders on that pair round to 5 places), and a string "1e-7" has no point at all
/// (Rails says 0). Rust returns the value's true decimal places (6 and 7). Every value Ruby writes in plain notation reads
/// as Rails reads it. (Gemini, the plan's example, passes `x.to_d.to_s`, which ActiveSupport writes in plain notation, so
/// its 1e-8 tick is 8 in Rails too; the recorded vectors show it.)
pub fn decimals(v: &Value) -> i64 {
    let text = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => match n.as_f64() { Some(f) if !(n.is_i64() || n.is_u64()) => ruby::float_to_s(f), _ => n.to_string() },
        Value::Null => String::new(),
        other => other.to_string(),
    };
    decimals_text(&text)
}

/// `decimals` over a value's text.
pub fn decimals_text(s: &str) -> i64 {
    if let Some(places) = exponent_places(s) { return places; }
    // str.sub(/\.?0+$/, ''): the trailing zeros, and a point just before them.
    let trimmed = s.trim_end_matches('0');
    let stripped = if trimmed.len() < s.len() { trimmed.strip_suffix('.').unwrap_or(trimmed) } else { s };
    if !stripped.contains('.') { return 0; }
    split(stripped, '.').last().map_or(0, |f| f.chars().count() as i64)
}

/// A decimal written as mantissa and exponent ("1.0e-06", "1e-7", "1.5E+3"): its true places.
fn exponent_places(s: &str) -> Option<i64> {
    let (mantissa, exp) = s.trim().split_once(['e', 'E'])?;
    let exp: i64 = exp.parse().ok()?;
    let digits = mantissa.strip_prefix(['-', '+']).unwrap_or(mantissa);
    let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
    if int.is_empty() && frac.is_empty() || !int.chars().chain(frac.chars()).all(|c| c.is_ascii_digit()) { return None; }
    Some((frac.trim_end_matches('0').len() as i64 - exp).max(0))
}

// --- Ruby's readings of a venue value ----------------------------------------------------------------------------------

fn row(v: [Value; 11]) -> Value {
    const KEYS: [&str; 10] = ["ticker", "base", "quote", "minimum_base_size", "minimum_quote_size", "maximum_base_size", "maximum_quote_size",
                              "base_decimals", "quote_decimals", "price_decimals"];
    let [ticker, base, quote, min_base, min_quote, max_base, max_quote, bd, qd, pd, enabled] = v;
    let mut m: Map<String, Value> = KEYS.iter().map(|k| k.to_string()).zip([ticker, base, quote, min_base, min_quote, max_base, max_quote, bd, qd, pd]).collect();
    m.insert("available".into(), json!(true));
    m.insert("trading_enabled".into(), enabled);
    Value::Object(m)
}

/// Utilities::Hash.dig_or_raise.
fn dig<'a>(v: &'a Value, keys: &[&str]) -> Result<&'a Value, String> {
    let found = keys.iter().fold(v, |cur, k| &cur[*k]);
    if found.is_null() { Err(format!("Key path {} not found", keys.join(" -> "))) } else { Ok(found) }
}

fn list(v: &Value) -> Result<&Vec<Value>, String> { v.as_array().ok_or_else(|| format!("undefined method 'map' for {}", ruby::raw(v))) }
fn some(v: Option<&Value>) -> Result<&Value, String> { v.ok_or_else(|| "undefined method '[]' for nil (a missing filter)".to_string()) }
fn string(v: &Value) -> Result<&str, String> { v.as_str().ok_or_else(|| format!("{} is not a string", ruby::raw(v))) }

/// `filters.find { |f| f['filterType'].in?(types) }`.
fn filter<'a>(filters: &'a Value, types: &[&str]) -> Result<Option<&'a Value>, String> { filter_in(list(filters)?, types) }
fn filter_in<'a>(filters: &'a [Value], types: &[&str]) -> Result<Option<&'a Value>, String> {
    Ok(filters.iter().find(|f| f["filterType"].as_str().is_some_and(|t| types.contains(&t))))
}

/// Ruby truthiness: all but nil and false.
fn truthy(v: &Value) -> bool { !matches!(v, Value::Null | Value::Bool(false)) }
/// `present?`: truthy and not a blank string.
fn present(v: &Value) -> bool { truthy(v) && v.as_str().is_none_or(|s| !ruby::blank(s)) }
/// `a || b`.
fn or<'a>(a: &'a Value, b: &'a Value) -> &'a Value { if truthy(a) { a } else { b } }
/// `"#{v}"`.
fn to_s(v: &Value) -> String { match v { Value::String(s) => s.clone(), Value::Null => String::new(), other => other.to_string() } }
/// `v&.upcase`.
fn upcase(v: &Value) -> Result<Value, String> { if v.is_null() { Ok(Value::Null) } else { Ok(json!(string(v)?.to_uppercase())) } }
/// String#split(sep): trailing empty fields dropped.
fn split(s: &str, sep: char) -> Vec<&str> {
    let mut parts: Vec<&str> = s.split(sep).collect();
    while parts.last().is_some_and(|p| p.is_empty()) { parts.pop(); }
    parts
}
/// `v.to_i` on nil, an Integer, a Float or a String.
fn to_i(v: &Value) -> Result<i64, String> {
    match v {
        Value::Null => Ok(0),
        Value::String(s) => Ok(ruby::to_i(s)),
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i64)).ok_or_else(|| format!("{n} has no to_i")),
        other => Err(format!("undefined method 'to_i' for {}", ruby::raw(other))),
    }
}
/// `v.to_d`: nil and a blank string are 0. A string that is not a decimal fails the venue (`ruby::json_to_d`), where
/// String#to_d would read its leading digits, or 0.
fn to_d(v: &Value) -> Result<BigDec, String> {
    match v {
        Value::Null => Ok(BigDec::zero()),
        Value::String(s) if ruby::blank(s) => Ok(BigDec::zero()),
        _ => ruby::json_to_d(v).map(|d| d.unwrap_or_else(BigDec::zero)).map_err(|e| format!("{e:?}")),
    }
}
/// `v.to_d` as `as_json` writes it: BigDecimal#to_s, plain.
fn d(v: &Value) -> Result<Value, String> { to_d(v).map(|x| dec(&x)) }
fn dec(x: &BigDec) -> Value { Value::String(x.to_s_f()) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_reads_as_rails_but_exponents_by_value() {
        for (v, n) in [(json!("0.00001000"), 5), (json!("1.00000000"), 0), (json!("100"), 0), (json!("10.0"), 0), (json!("0.10"), 1),
                       (json!(0.01), 2), (json!(5), 0), (Value::Null, 0), (json!("5."), 1), (json!(0.0001), 4)] {
            assert_eq!(decimals(&v), n, "{v}");
        }
        // D-CG-DECIMALS-EXPONENT: Rails reads these as 5, 5, 5 and 0.
        for (v, n) in [(json!(1e-6), 6), (json!(1e-8), 8), (json!(1.5e-11), 12), (json!("1e-7"), 7), (json!(1e-5), 5), (json!(1e20), 0)] {
            assert_eq!(decimals(&v), n, "{v}");
        }
        assert_eq!(decimals(&json!("none")), 0, "an 'e' that is no exponent is Rails' count");
    }
}
