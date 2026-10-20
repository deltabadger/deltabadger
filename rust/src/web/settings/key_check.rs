//! The key checks of the venues that report what a key may do: Exchanges::Kraken#key_permission_validity (GetApiKeyInfo's
//! permission list against KEY_PERMISSIONS) and Exchanges::Binance#trading_key_validity / #reading_key_validity (the
//! apiRestrictions flags). Each verdict is a pure function of the venue's answer, replayed against the recorded Rails
//! verdicts (tests/fixtures/key_verdicts.json); `check` sends the one request and reads it. Exchanges::Kucoin#get_api_key_validity
//! probes instead (a cancel of the zero order id, or a trading-account read for a withdrawal key). Exchanges::BinanceUs has no
//! apiRestrictions: a signed account read, then (for a key that may trade or withdraw) a cancel of an order that cannot
//! exist (`binance_us_check`).
use super::validator::{Diagnostic, Validity};
use crate::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use crate::web::i18n;
use serde_json::Value;

/// What Rails' check answers: `Success(true|false)`, `Success({missing_permissions:, forbidden_permissions:})`, or a Failure.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict { Valid(bool), Permissions { missing: Vec<String>, forbidden: Vec<String> }, Failure(String) }

impl Verdict {
    /// ApiKey#validate_credentials! / #update_status!: a permission problem is an incorrect key that names its flags.
    pub fn validity(self, status: Option<u16>, body: &str) -> Validity {
        match self {
            Verdict::Valid(true) => Validity::Correct,
            Verdict::Valid(false) => Validity::Incorrect,
            Verdict::Permissions { missing, forbidden } => Validity::Permissions { missing, forbidden },
            Verdict::Failure(text) => Validity::Pending(Diagnostic { text, status, body: body.into() }),
        }
    }
}

/// Exchanges::Kraken::ERRORS[:invalid_key].
pub const KRAKEN_INVALID_KEY: [&str; 2] = ["EAPI:Invalid key", "EAPI:Invalid signature"];
/// Exchanges::Binance::ERROR_CODES[:invalid_key].
pub const BINANCE_INVALID_KEY: [i64; 3] = [-2008, -2014, -2015];
pub const KRAKEN_PATH: &str = "/0/private/GetApiKeyInfo";
pub const BINANCE_PATH: &str = "/sapi/v1/account/apiRestrictions";

/// Exchanges::Kraken::KEY_PERMISSIONS[key_type]: (required, forbidden). key_type as stored: 0 trading, 1 withdrawal, 2 read_only.
pub fn kraken_permissions(kind: i64) -> (&'static [&'static str], &'static [&'static str]) {
    match kind {
        1 => (&["query-funds", "withdraw-funds"], &["query-open-trades", "query-closed-trades", "modify-trades", "close-trades"]),
        2 => (&["query-funds", "query-ledger"], &["withdraw-funds"]),
        _ => (&["query-funds", "query-open-trades", "query-closed-trades", "modify-trades", "close-trades", "query-ledger"], &["withdraw-funds"]),
    }
}

/// Faraday's message for an answer it raised on (raise_error), as honeymaker returns it: the body, or the status line.
fn raised(response: &HttpResponse, method: &str, url: &str) -> String {
    if response.body.is_empty() { format!("the server responded with status {} for {method} {url}", response.status) } else { response.body.clone() }
}

fn transport(error: &TransportError) -> String {
    match error { TransportError::NotSent(m) | TransportError::MaybeSent(m) | TransportError::Permanent(m) => m.clone() }
}

/// Ruby truthiness of a decoded value.
fn truthy(value: &Value) -> bool { !matches!(value, Value::Null | Value::Bool(false)) }

/// Exchanges::Kraken#key_permission_validity on GetApiKeyInfo's answer.
pub fn kraken_verdict(kind: i64, answer: &Result<HttpResponse, TransportError>) -> Verdict {
    let response = match answer { Ok(response) => response, Err(error) => return Verdict::Failure(transport(error)) };
    if !(200..300).contains(&response.status) { return Verdict::Failure(raised(response, "POST", &format!("{}{KRAKEN_PATH}", crate::venue::kraken::URL))); }
    // An empty or non-JSON 200 parses to nil or a String (or not at all): no verdict either way.
    let data = match serde_json::from_str::<Value>(&response.body) {
        Ok(Value::Object(data)) => data,
        Ok(_) => return Verdict::Failure("Kraken sent an unreadable answer to the key check".into()),
        Err(_) if response.body.trim().is_empty() => return Verdict::Failure("Kraken sent an unreadable answer to the key check".into()),
        Err(_) => return Verdict::Failure(response.body.clone()),
    };
    // `Array(result.data['error'])`.
    let errors: Vec<Value> = match data.get("error") { None | Some(Value::Null) => vec![], Some(Value::Array(items)) => items.clone(), Some(other) => vec![other.clone()] };
    if errors.iter().any(|error| error.as_str().is_some_and(|text| KRAKEN_INVALID_KEY.contains(&text))) { return Verdict::Valid(false); }
    if errors.iter().any(truthy) {
        return Verdict::Failure(errors.iter().map(|e| e.as_str().map_or_else(|| e.to_string(), str::to_string)).collect::<Vec<_>>().join(", "));
    }
    // ponytail: a `result` that is not an object makes Ruby's `dig` raise (a 500); here it is no permission list.
    let Some(Value::Array(held)) = data.get("result").and_then(|result| result.get("permissions")) else {
        return Verdict::Failure("Kraken returned no permission list for this key".into());
    };
    let holds = |flag: &str| held.iter().any(|value| value.as_str() == Some(flag));
    let (required, forbidden) = kraken_permissions(kind);
    let missing: Vec<String> = required.iter().filter(|flag| !holds(flag)).map(|flag| flag.to_string()).collect();
    let forbidden: Vec<String> = forbidden.iter().filter(|flag| holds(flag)).map(|flag| flag.to_string()).collect();
    if missing.is_empty() && forbidden.is_empty() { Verdict::Valid(true) } else { Verdict::Permissions { missing, forbidden } }
}

/// Exchanges::Binance#trading_key_validity (`reading` false) or #reading_key_validity on apiRestrictions' answer.
pub fn binance_verdict(kind: i64, reading: bool, answer: &Result<HttpResponse, TransportError>, url: &str) -> Verdict {
    let response = match answer { Ok(response) => response, Err(error) => return Verdict::Failure(transport(error)) };
    if !(200..300).contains(&response.status) {
        // #parse_error_code: the failure text's JSON `code`, compared as Ruby's Integer#== does (a "-2015" string is not one).
        let text = raised(response, "GET", url);
        let code = serde_json::from_str::<Value>(&text).ok().and_then(|value| value.get("code").cloned());
        let invalid = code.as_ref().and_then(Value::as_f64).is_some_and(|code| BINANCE_INVALID_KEY.iter().any(|known| *known as f64 == code));
        return if invalid { Verdict::Valid(false) } else { Verdict::Failure(text) };
    }
    let data = match serde_json::from_str::<Value>(&response.body) {
        Ok(Value::Object(data)) => data,
        Err(_) if !response.body.trim().is_empty() => return Verdict::Failure(response.body.clone()),
        _ => return Verdict::Failure("Binance sent an unreadable answer to the key check".into()),
    };
    let is = |flag: &str, expected: bool| data.get(flag) == Some(&Value::Bool(expected));
    if reading { return Verdict::Valid(is("enableReading", true) && !is("enableWithdrawals", true)); }
    let common = is("ipRestrict", true) && is("enableFixApiTrade", false) && is("enableFixReadOnly", false) && is("enableFutures", false)
        && is("enableInternalTransfer", false) && is("enableMargin", false) && is("enablePortfolioMarginTrading", false) && is("enableReading", true)
        && is("enableVanillaOptions", false) && is("permitsUniversalTransfer", false);
    Verdict::Valid(if kind == 1 { common && is("enableWithdrawals", true) && is("enableSpotAndMarginTrading", false) }
                   else { common && is("enableSpotAndMarginTrading", true) && is("enableWithdrawals", false) })
}

/// Exchanges::Kucoin::KEY_CHECK_REJECTED and KEY_CHECK_INCONCLUSIVE.
pub const KUCOIN_REJECTED: [&str; 5] = ["400003", "400004", "400005", "400006", "400007"];
pub const KUCOIN_INCONCLUSIVE: [&str; 7] = ["400001", "400002", "429000", "500000", "230005", "404000", "200002"];
/// The order id #get_api_key_validity cancels: no order has it, so only the key is judged.
pub const KUCOIN_PROBE_ORDER: &str = "000000000000000000000000";

/// Exchanges::Kucoin#get_api_key_validity on its probe's answer (`kind` 1, a withdrawal key, read its trading account): the
/// envelope's or error body's code and msg (#response_field) decide; any other venue code passes a trading probe.
pub fn kucoin_verdict(kind: i64, answer: &Result<HttpResponse, TransportError>, method: &str, url: &str) -> Verdict {
    let (body, failure) = match answer {
        Err(error) => (None, Some(transport(error))),
        Ok(response) if (200..300).contains(&response.status) => match serde_json::from_str::<Value>(&response.body) {
            Ok(value) => (Some(value), None),
            // Faraday's JSON middleware leaves an empty body nil, and raises on one it cannot parse (#with_rescue: its body).
            Err(_) if response.body.trim().is_empty() => (None, None),
            Err(_) => (None, Some(response.body.clone())),
        },
        Ok(response) => { let text = raised(response, method, url); (serde_json::from_str::<Value>(&text).ok(), Some(text)) }
    };
    let field = |name: &str| body.as_ref().filter(|b| b.is_object()).and_then(|b| b.get(name)).filter(|v| !v.is_null())
        .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string));
    let (code, message) = (field("code"), field("msg").unwrap_or_default());
    if code.as_deref() == Some("200000") { return Verdict::Valid(true); }
    if code.as_deref().is_some_and(|c| KUCOIN_REJECTED.contains(&c)) || crate::engine::venue_rules::KUCOIN.is_invalid_key(std::slice::from_ref(&message)) {
        return Verdict::Valid(false);
    }
    if code.is_none() || code.as_deref().is_some_and(|c| KUCOIN_INCONCLUSIVE.contains(&c)) || kind == 1 {
        return Verdict::Failure(failure.unwrap_or_else(|| if message.is_empty() { format!("KuCoin answered code {}", code.map_or("nil".into(), |c| format!("{c:?}"))) } else { message }));
    }
    Verdict::Valid(true)
}

/// Exchange#get_read_api_key_validity on KuCoin (a reading key): #get_balances(asset_ids: []), its trading-account read. Balances
/// only under the success code with `data`; else the failure Exchanges::Kucoin#get_balances writes, and a key the venue's
/// invalid_key words condemn (#condemning_invalid_key_error?) is incorrect.
pub fn kucoin_read_verdict(answer: &Result<HttpResponse, TransportError>, method: &str, url: &str) -> Verdict {
    let text = |v: &Value| v.as_str().map_or_else(|| v.to_string(), str::to_string);
    let failure = match answer {
        Err(error) => transport(error),
        Ok(response) if !(200..300).contains(&response.status) => raised(response, method, url),
        Ok(response) => match serde_json::from_str::<Value>(&response.body) {
            Ok(Value::Object(body)) => {
                let code = body.get("code").filter(|v| !v.is_null()).map(text);
                if code.as_deref() == Some("200000") && body.get("data").is_some_and(|data| !data.is_null()) {
                    // The balances, read as #get_balances walks them (each one's `currency` first): any other shape raises there.
                    let readable = match &body["data"] {
                        Value::Array(balances) => balances.iter().all(|balance| balance.get("currency").is_some_and(|currency| !currency.is_null())),
                        Value::Object(pairs) => pairs.is_empty(),
                        _ => false,
                    };
                    return if readable { Verdict::Valid(true) } else { Verdict::Failure("KuCoin sent balances it cannot read".into()) };
                }
                let parts: Vec<String> = ["code", "msg"].iter().filter_map(|name| body.get(*name).filter(|v| !v.is_null()).map(text)).collect();
                format!("Failed to get KuCoin balances: {}", parts.join(" "))
            }
            // ponytail: Ruby raises on an envelope that is not a Hash (a 500); here it is no verdict.
            _ => "KuCoin sent an unreadable answer to the key check".into(),
        },
    };
    if crate::engine::venue_rules::KUCOIN.is_invalid_key(std::slice::from_ref(&failure)) { Verdict::Valid(false) } else { Verdict::Failure(failure) }
}

/// #get_api_key_validity's probe: honeymaker's #cancel_order of the zero id, or #get_accounts(type: 'trade') for a withdrawal key
/// and for a reading key's balance read.
pub fn kucoin_request(base: &str, kind: i64) -> HttpRequest {
    let (method, path, query) = if kind != 0 { ("GET", "/api/v1/accounts".to_string(), vec![("type", "trade".to_string())]) }
                                else { ("DELETE", format!("/api/v1/orders/{KUCOIN_PROBE_ORDER}"), vec![]) };
    HttpRequest { method, base: base.into(), path, query, body: None, not_after: None }
}

/// Honeymaker's GetApiKeyInfo call: a private POST with only its nonce (added and signed by the Kraken wire at send time).
pub fn kraken_request(base: &str) -> HttpRequest {
    HttpRequest { method: "POST", base: base.into(), path: KRAKEN_PATH.into(), query: vec![], body: Some(Value::Object(Default::default())), not_after: None }
}

/// Honeymaker's api_description: a signed GET with the gem's default recvWindow.
pub fn binance_request<T: Transport>(venue: &crate::venue::binance::BinanceVenue<T>, now: chrono::DateTime<chrono::Utc>) -> HttpRequest {
    venue.signed("GET", BINANCE_PATH, vec![("recvWindow", crate::venue::binance::PLACE_RECV_WINDOW.into())], now)
}

/// Exchanges::BinanceUs::TRADE_PROBE: a cancel of an order that cannot exist (ETHBTC is listed there).
pub const BINANCE_US_PROBE: [(&str, &str); 2] = [("symbol", "ETHBTC"), ("orderId", "9999999999")];
/// What Exchanges::BinanceUs reads a key with: GET /api/v3/account (Binance's read recvWindow), and the probe (the gem's).
pub const BINANCE_US_ACCOUNT_PATH: &str = "/api/v3/account";
pub const BINANCE_US_PROBE_PATH: &str = "/api/v3/order";

/// Ruby's `#to_i` of a decoded `code` (an Integer, a Float truncated, a String's leading digits); None where Ruby raises.
fn code_to_i(code: &Value) -> Option<i64> {
    match code { Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)), Value::String(s) => Some(crate::ruby::to_i(s)), _ => None }
}

/// #parse_error_code of a failure's text: the JSON object's `code` (any type), else none.
fn error_code(text: &str) -> Option<Value> {
    serde_json::from_str::<Value>(text).ok().and_then(|v| v.as_object().and_then(|o| o.get("code").cloned())).filter(|c| !c.is_null())
}

/// `code == n` as Ruby compares a decoded number with an Integer (a String never equals one).
fn code_is(code: Option<&Value>, n: i64) -> bool { code.and_then(Value::as_f64).is_some_and(|c| c == n as f64) }

/// An answer as honeymaker hands it to the check: the 2xx body decoded (Ok(None) when it is not a JSON object), or the
/// failure's text.
fn decoded(answer: &Result<HttpResponse, TransportError>, method: &str, url: &str) -> Result<Option<serde_json::Map<String, Value>>, String> {
    match answer {
        Err(error) => Err(transport(error)),
        Ok(response) if !(200..300).contains(&response.status) => Err(raised(response, method, url)),
        Ok(response) => match serde_json::from_str::<Value>(&response.body) {
            Ok(Value::Object(data)) => Ok(Some(data)),
            Err(_) if !response.body.trim().is_empty() => Err(response.body.clone()),
            _ => Ok(None),
        },
    }
}

/// Exchanges::BinanceUs#reading_key_validity on the account read's answer: a readable account is a key that reads; a
/// Binance-style error envelope inside a 200 is the venue's answer (an invalid-key code: incorrect; any other: no verdict).
pub fn binance_us_read_verdict(answer: &Result<HttpResponse, TransportError>, url: &str) -> Verdict {
    let unreadable = || Verdict::Failure("Binance.US sent an unreadable answer to the key check".into());
    match decoded(answer, "GET", url) {
        Err(text) => if error_code(&text).as_ref().and_then(Value::as_f64).is_some_and(|c| BINANCE_INVALID_KEY.iter().any(|k| *k as f64 == c)) { Verdict::Valid(false) } else { Verdict::Failure(text) },
        Ok(None) => unreadable(),
        Ok(Some(data)) => match data.get("code").filter(|c| !matches!(c, Value::Null)) {
            None => Verdict::Valid(true),
            Some(code) => match code_to_i(code) {
                Some(n) if BINANCE_INVALID_KEY.contains(&n) => Verdict::Valid(false),
                _ => Verdict::Failure(format!("Binance.US answered the key check with code {}", code.as_str().map_or_else(|| code.to_string(), str::to_string))),
            },
        },
    }
}

/// Exchanges::BinanceUs#trading_key_validity on the probe's answer, the account read having passed: -2011 (unknown order),
/// or a cancel that went through, is a key that may trade; an invalid-key code one that may not; anything else no verdict.
/// A withdrawal key (`kind` 1) is held to the opposite: it reads and cannot trade.
pub fn binance_us_probe_verdict(kind: i64, answer: &Result<HttpResponse, TransportError>, url: &str) -> Verdict {
    let can_trade = match decoded(answer, "DELETE", url) {
        Err(text) => {
            let code = error_code(&text);
            if code_is(code.as_ref(), -2011) { true }
            else if BINANCE_INVALID_KEY.iter().any(|k| code_is(code.as_ref(), *k)) { false }
            else { return Verdict::Failure(text) }
        }
        Ok(None) => return Verdict::Failure("Binance.US sent an unreadable answer to the key check".into()),
        // `probe.data['code'].to_i` when the code is truthy.
        Ok(Some(data)) => match data.get("code").filter(|c| !matches!(c, Value::Null | Value::Bool(false))) {
            None => true,
            Some(code) => match code_to_i(code) {
                Some(-2011) => true,
                Some(n) if BINANCE_INVALID_KEY.contains(&n) => false,
                Some(n) => return Verdict::Failure(format!("Binance.US answered the key check with code {n}")),
                None => return Verdict::Failure(format!("Binance.US answered the key check with code {code}")),
            },
        },
    };
    Verdict::Valid(if kind == 1 { !can_trade } else { can_trade })
}

/// Exchanges::BinanceUs's key check of a key of `kind` (`reading`: #get_read_api_key_validity, the account read alone),
/// sent through `transport`: the account read, then the probe when the read passed and the key is not read-only. The last
/// answer (for the diagnostics) and the verdict.
pub async fn binance_us_check<T: Transport>(transport: &T, venue: &crate::venue::binance::BinanceVenue<T>, kind: i64, reading: bool)
                                           -> (Result<HttpResponse, TransportError>, Verdict) {
    use crate::venue::binance::{PLACE_RECV_WINDOW, READ_RECV_WINDOW};
    let read = venue.signed("GET", BINANCE_US_ACCOUNT_PATH, vec![("omitZeroBalances", "true".into()), ("recvWindow", READ_RECV_WINDOW.into())], transport.now());
    let answer = transport.send(&read).await;
    let verdict = binance_us_read_verdict(&answer, read.url().as_str());
    if reading || verdict != Verdict::Valid(true) { return (answer, verdict); }
    let mut query: Vec<(&'static str, String)> = BINANCE_US_PROBE.iter().map(|(k, v)| (*k, v.to_string())).collect();
    query.push(("recvWindow", PLACE_RECV_WINDOW.into()));
    let probe = venue.signed("DELETE", BINANCE_US_PROBE_PATH, query, transport.now());
    let answer = transport.send(&probe).await;
    let verdict = binance_us_probe_verdict(kind, &answer, probe.url().as_str());
    (answer, verdict)
}

/// The key check of `venue` (an exchanges.type this module ports) for a key of `kind`, sent through `client` to `base`.
pub async fn check(venue: &str, base: &str, client: reqwest::Client, credentials: &crate::crypto::Credentials, kind: i64) -> Validity {
    use crate::venue::http::ReqwestTransport;
    let redaction = credentials.redaction_values.clone();
    use crate::venue::registry::KeyCheck;
    let scheme = crate::venue::registry::row(venue).and_then(|r| r.key_check);
    let (answer, verdict) = if scheme == Some(KeyCheck::Kraken) {
        let transport = ReqwestTransport::new(client, credentials.key.clone(), credentials.secret.clone()).kraken().with_sensitive_values(redaction);
        let answer = transport.send(&kraken_request(base)).await;
        let verdict = kraken_verdict(kind, &answer);
        (answer, verdict)
    } else if scheme == Some(KeyCheck::Kucoin) {
        let passphrase = credentials.passphrase.clone().unwrap_or_default();
        let transport = ReqwestTransport::kucoin(client, credentials.key.clone(), credentials.secret.clone(), passphrase).with_sensitive_values(redaction);
        let request = kucoin_request(base, kind);
        let answer = transport.send(&request).await;
        let verdict = if kind == 2 { kucoin_read_verdict(&answer, request.method, request.url().as_str()) }
                      else { kucoin_verdict(kind, &answer, request.method, request.url().as_str()) };
        (answer, verdict)
    } else if scheme == Some(KeyCheck::BinanceUs) {
        let transport = ReqwestTransport::binance(client, credentials.key.clone(), credentials.secret.clone()).with_sensitive_values(redaction);
        let venue = crate::venue::binance::BinanceVenue::new(transport.clone(), base, &credentials.secret);
        binance_us_check(&transport, &venue, kind, kind == 2).await
    } else {
        let transport = ReqwestTransport::binance(client, credentials.key.clone(), credentials.secret.clone()).with_sensitive_values(redaction);
        let venue = crate::venue::binance::BinanceVenue::new(transport.clone(), base, &credentials.secret);
        let request = binance_request(&venue, transport.now());
        let answer = transport.send(&request).await;
        let verdict = binance_verdict(kind, kind == 2, &answer, request.url().as_str());
        (answer, verdict)
    };
    let (status, body) = answer.as_ref().map_or((None, String::new()), |response| (Some(response.status), response.body.clone()));
    let body = credentials.venue_text(&body);
    match verdict {
        Verdict::Failure(text) => Verdict::Failure(credentials.venue_text(&text)),
        other => other,
    }.validity(status, &body)
}

/// Whether the locale has `key` itself or a list under it, no fallback (`I18n.exists?(key, locale:, fallback: false)`).
pub fn exists_in(locale: &str, key: &str) -> bool {
    let full = format!("{locale}.{key}");
    let nested = format!("{full}.");
    i18n::all().iter().any(|(name, _)| *name == full || name.starts_with(&nested))
}

/// BotHelper#api_key_instructions_key.
fn instructions_key(name_id: &str, kind: i64) -> String {
    match kind {
        1 => format!("withdrawal_api.{name_id}.instructions"),
        2 if exists_in(i18n::DEFAULT, &format!("read_only_api.{name_id}.instructions")) => format!("read_only_api.{name_id}.instructions"),
        _ => format!("bot.api.{name_id}.instructions"),
    }
}

/// BotHelper#api_key_permission_label: a flag in the words of the steps it is listed in, in the locale those steps render in.
fn permission_label(locale: &str, name_id: &str, kind: i64, flag: &str) -> String {
    let steps = if exists_in(locale, &instructions_key(name_id, kind)) { locale } else { i18n::DEFAULT };
    i18n::text_chain(steps, &[format!("api_key_permissions.{name_id}.{}", flag.replace('-', "_"))], &[]).unwrap_or_else(|| flag.to_string())
}

/// BotHelper#api_key_permission_message: what the key lacks, what it must not hold, and what to do.
pub fn permission_message(locale: &str, name_id: &str, exchange: &str, kind: i64, missing: &[String], forbidden: &[String]) -> String {
    let labels = |flags: &[String]| flags.iter().map(|flag| permission_label(locale, name_id, kind, flag)).collect::<Vec<_>>().join(", ");
    let mut parts = vec![];
    if !missing.is_empty() { parts.push(i18n::text(locale, "errors.api_key_missing_permissions", &[("permissions", i18n::Arg::Text(&labels(missing)))])); }
    if !forbidden.is_empty() { parts.push(i18n::text(locale, "errors.api_key_forbidden_permissions", &[("permissions", i18n::Arg::Text(&labels(forbidden)))])); }
    parts.push(i18n::text(locale, "errors.api_key_update_permissions", &[("exchange", i18n::Arg::Text(exchange))]));
    parts.join(" ")
}

/// BotHelper#whitelist_ip_html_for: the host of the venue's proxy (ExchangeProxy.for), the IP its keys are bound to, or the
/// fallback words when the install talks to the venue directly.
pub fn whitelist_ip_html(locale: &str, proxy: Option<&str>) -> String {
    match proxy.and_then(|url| reqwest::Url::parse(url).ok()).and_then(|url| url.host_str().map(str::to_string)) {
        Some(host) => format!("<code>{host}</code>"),
        None => i18n::t(locale, "bot.api.whitelist_ip_fallback_html", &[]),
    }
}

/// BotHelper#render_instructions_from(prefix, exchange): the walkthrough `prefix.<name_id>.instructions` as the locale has it
/// (English when the locale has none), a paragraph when it is one line, else the steps as a list whose second level is
/// numbered and deeper ones bulleted. `None` when no locale has it.
pub fn instructions_html(locale: &str, prefix: &str, name_id: &str, exchange_name: &str, whitelist_ip: &str) -> Option<String> {
    let key = format!("{prefix}.{name_id}.instructions");
    let locale = if exists_in(locale, &key) { locale } else if exists_in(i18n::DEFAULT, &key) { i18n::DEFAULT } else { return None };
    let leaf = |path: &str| i18n::all().iter().find(|(name, _)| *name == format!("{locale}.{path}")).map(|(_, text)| *text);
    let text = |raw: &str| raw.replace("%{exchange_link}", exchange_name).replace("%{whitelist_ip}", whitelist_ip);
    if let Some(line) = leaf(&key) { return Some(format!("<p>{}</p>", text(line))); }
    fn steps(path: &str, level: usize, leaf: &dyn Fn(&str) -> Option<&'static str>, text: &dyn Fn(&str) -> String) -> String {
        let mut out = String::new();
        for index in 0.. {
            let item = format!("{path}.{index}");
            let Some(line) = leaf(&format!("{item}.text_html")) else { break };
            out.push_str("<li>");
            out.push_str(&text(line));
            let nested = steps(&format!("{item}.sub_instructions"), level + 1, leaf, text);
            if !nested.is_empty() {
                let tag = if level == 1 { "ol" } else { "ul" };
                out.push_str(&format!("<{tag}>{nested}</{tag}>"));
            }
            out.push_str("</li>");
        }
        out
    }
    Some(format!("<ol class=\"set-api__list\">{}</ol>", steps(&key, 1, &leaf, &text)))
}
