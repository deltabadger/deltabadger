//! A `MarketData` that replays a scenario's recorded answers: the same script the Rails harness
//! (script/rust/figures.rb) serves beneath Clients::Alpaca, Clients::MarketData and Clients::Coingecko at the
//! Faraday adapter. It builds the request each of those clients would send and parses the answer as each parses it,
//! so the requests can be compared with Rails' and the parsing runs on the very bodies Rails parsed.
//! A request the script does not answer is recorded in `gaps` and fails the read.
use super::at::At;
use super::db::Ticker;
use super::dec::Dec;
use super::market::{Failure, Fetch, MarketData, Member, Quoted, Venue};
use super::num::{Num, NumError};
use chrono::DateTime;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;

const ALPACA: &str = "GET data.alpaca.markets";
const HOSTED: &str = "GET data-api:3000/api/v1";
const COINGECKO: &str = "GET api.coingecko.com/api/v3";

pub struct Scripted {
    script: Value,
    /// AppConfig.market_data_provider: `deltabadger` (the hosted API), `coingecko`, or none configured.
    provider: Option<String>,
    /// Every request asked for, whole: method, host, path and the sorted query.
    pub requests: RefCell<Vec<String>>,
    pub gaps: RefCell<Vec<String>>,
}

/// Exchanges::Alpaca#get_candles' names for a timeframe; anything else is a day.
fn timeframe_name(seconds: i64) -> &'static str {
    match seconds { 60 => "1Min", 300 => "5Min", 900 => "15Min", 1800 => "30Min", 3600 => "1Hour", 14_400 => "4Hour", 604_800 => "1Week", 2_592_000 => "1Month", _ => "1Day" }
}

impl Scripted {
    pub fn new(script: &Value, provider: Option<&str>) -> Self {
        Self { script: script.clone(), provider: provider.map(str::to_string), requests: RefCell::default(), gaps: RefCell::default() }
    }

    /// `key` is what the reply is scripted under, `query` the rest of the request. The body of a 2xx answer.
    fn get(&self, key: &str, line: String) -> Fetch<Value> {
        self.requests.borrow_mut().push(line);
        let Some(reply) = self.script.get(key) else {
            self.gaps.borrow_mut().push(key.to_string());
            return Err(Failure::Raised(format!("unscripted market-data call {key}")));
        };
        let message = reply["message"].as_str().unwrap_or_default();
        match reply["network"].as_str() {
            // Client.network_failure: a failure a retry can fix is raised; a certificate failure is returned.
            Some("transient") => return Err(Failure::Raised(format!("Client::TransientNetworkError: {message}"))),
            Some(_) => return Err(Failure::Failed(message.to_string())),
            None => {}
        }
        let status = reply["status"].as_u64().unwrap_or(200);
        if !(200..300).contains(&status) { return Err(Failure::Failed(format!("HTTP {status}"))); }
        match &reply["body"] {
            Value::String(text) => serde_json::from_str(text).map_err(|_| Failure::Failed(format!("Unreadable response (HTTP {status})"))),
            body => Ok(body.clone()),
        }
    }

    fn alpaca(&self, path: &str, picked: &[(&str, &str)], query: &[(&str, &str)]) -> Fetch<Value> {
        let join = |pairs: &[(&str, &str)]| pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
        let with = |pairs: &[(&str, &str)]| if pairs.is_empty() { format!("{ALPACA}{path}") } else { format!("{ALPACA}{path}?{}", join(pairs)) };
        self.get(&with(picked), with(query))
    }
}

impl MarketData for Scripted {
    /// Exchanges::Alpaca#get_tickers_prices: stocks by their snapshots' latest trade, crypto pairs (a `/` in the
    /// code) by their latest trade; a missing figure reads as zero, as `nil.to_d` does.
    fn prices(&self, _venue: &Venue, symbols: &[String]) -> Fetch<Vec<(String, Member<Dec>)>> {
        let mut prices: Vec<(String, Member<Dec>)> = vec![];
        let mut place: HashMap<String, usize> = HashMap::new(); // an index bot asks for every ticker of the venue
        let sorted = |crypto: bool| { let mut list: Vec<&str> = symbols.iter().filter(|s| s.contains('/') == crypto).map(String::as_str).collect(); list.sort_unstable(); list.join(",") };
        let (stocks, crypto) = (sorted(false), sorted(true));
        let mut put = |code: &str, price: Member<Dec>| match place.get(code) { Some(&at) => prices[at].1 = price, None => { place.insert(code.to_string(), prices.len()); prices.push((code.to_string(), price)); } };
        if !stocks.is_empty() {
            let body = self.alpaca("/v2/stocks/snapshots", &[], &[("symbols", &stocks)])?;
            for (code, snapshot) in body.as_object().into_iter().flatten() { put(code, Dec::to_d(&snapshot["latestTrade"]["p"])); }
        }
        if !crypto.is_empty() {
            let body = self.alpaca("/v1beta3/crypto/us/latest/trades", &[], &[("symbols", &crypto)])?;
            for (code, trade) in body["trades"].as_object().into_iter().flatten() { put(code, Dec::to_d(&trade["p"])); }
        }
        Ok(prices)
    }

    /// Exchanges::Alpaca#get_candles: a stock's bars by its symbol (restated ones with `adjustment=split`), a crypto
    /// pair's by its pair. `start` is the instant without its fraction, as Time#iso8601 writes it.
    fn candles(&self, _venue: &Venue, ticker: &Ticker, since: At, timeframe: i64, restated: bool) -> Fetch<Vec<(At, Dec)>> {
        let start = since.utc().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let name = timeframe_name(timeframe);
        let bars = if ticker.base_category.as_deref() == Some("Cryptocurrency") {
            let pair = [("symbols", ticker.ticker.as_str())];
            let body = self.alpaca("/v1beta3/crypto/us/bars", &pair, &[("start", &start), pair[0], ("timeframe", name)])?;
            body["bars"][&ticker.ticker].clone()
        } else {
            let path = format!("/v2/stocks/{}/bars", ticker.base);
            let adjustment = [("adjustment", "split")];
            let rest = [("limit", "10000"), ("start", start.as_str()), ("timeframe", name)];
            let body = if restated { self.alpaca(&path, &adjustment, &[adjustment[0], rest[0], rest[1], rest[2]])? } else { self.alpaca(&path, &[], &rest)? };
            body["bars"].clone()
        };
        bars.as_array().into_iter().flatten().map(|bar| {
            let at = bar["t"].as_str().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).and_then(|t| At::from_utc(t.to_utc()));
            let at = at.ok_or_else(|| Failure::Failed(format!("a bar without a time: {bar}")))?;
            Ok((at, Dec::to_d(&bar["o"])?))
        }).collect()
    }

    fn exchange_rates(&self) -> Fetch<Vec<(String, Member<Num>)>> {
        let rates = match self.provider.as_deref() {
            Some("deltabadger") => self.get(&format!("{HOSTED}/exchange_rates"), format!("{HOSTED}/exchange_rates"))?["data"].clone(),
            Some("coingecko") => self.get(&format!("{COINGECKO}/exchange_rates"), format!("{COINGECKO}/exchange_rates"))?["rates"].clone(),
            _ => return Err(Failure::Failed("No market data provider configured".into())),
        };
        let mut out = vec![];
        for (currency, rate) in rates.as_object().into_iter().flatten() {
            match Num::from_json(&rate["value"]) {
                Ok(Some(value)) => out.push((currency.clone(), Ok(value))),
                Ok(None) if rate["value"].is_null() => {} // no value: the currency has no rate
                // Ruby would divide a String by a String and raise NoMethodError somewhere this plan did not measure.
                Ok(None) => out.push((currency.clone(), Err(NumError::NotANumber))),
                Err(error) => out.push((currency.clone(), Err(error))),
            }
        }
        Ok(out)
    }

    fn coin_price(&self, coin_id: &str, currency: &str) -> Fetch<Quoted> {
        match self.provider.as_deref() {
            Some("deltabadger") => {
                let key = format!("{HOSTED}/prices?coin_ids={coin_id}&vs_currencies={currency}");
                let body = self.get(&key, key.clone())?;
                Quoted::from_json(&body["data"][coin_id][currency])?.ok_or_else(|| Failure::Failed(format!("Price not found for {coin_id} in {currency}")))
            }
            Some("coingecko") => {
                let key = format!("{COINGECKO}/simple/price?ids={coin_id}&vs_currencies={currency}");
                let line = format!("{COINGECKO}/simple/price?ids={coin_id}&include_24hr_change=false&include_24hr_vol=false&include_last_updated_at=false&include_market_cap=false&precision=full&vs_currencies={currency}");
                let body = self.get(&key, line)?;
                // Utilities::Hash.dig_or_raise, inside Coingecko#get_price.
                Quoted::from_json(&body[coin_id][currency])?.ok_or_else(|| Failure::Raised(format!("KeyError: Key path {coin_id} -> {currency} not found")))
            }
            _ => Err(Failure::Failed("No market data provider configured".into())),
        }
    }
}
