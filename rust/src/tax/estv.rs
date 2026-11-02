//! The ESTV Kursliste year-end values of crypto assets (ictax; D-TAX-CH-ESTV), stored in `estv_crypto_values`: CHF per unit,
//! or NULL where ESTV was asked and lists no such token. The Swiss report reads them; the job fills them for a finished
//! year before the report (`ensure_loaded`), as the ČNB's rates are filled. ictax's JSON API is the one its own web app
//! calls (undocumented, verified 2026-11-02): a session (`session.json`: the CSRF token, and the cookies it sets), the
//! listed tokens (`tokens.json`), then the values on a day (`tokenGadget.json`, a POST carrying both).
use crate::figures::{dec::Dec, FiguresError};
use crate::jobs::Db;
use chrono::{Datelike, NaiveDate};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

pub const ICTAX_BASE: &str = "https://www.ictax.admin.ch/extern/api";

/// The stored values by (year, symbol): `Some(value)`, or `None` where ESTV lists no such token.
#[derive(Clone, Debug, Default)]
pub struct Estv { values: HashMap<(i32, String), Option<Dec>> }

impl Estv {
    pub fn load(c: &Connection) -> Result<Estv, FiguresError> {
        let mut s = c.prepare("SELECT year, symbol, value FROM estv_crypto_values")?;
        let mut q = s.query([])?;
        let mut values = HashMap::new();
        while let Some(r) = q.next()? {
            crate::figures::budget::charge(1, 0)?;
            values.insert((r.get(0)?, r.get(1)?), Dec::from_sql(r.get_ref(2)?)?);
        }
        Ok(Estv { values })
    }

    /// D-TAX-CH-ESTV's three states: `None` no row (unresolved), `Some(None)` not listed, `Some(Some(v))` the value.
    pub fn value(&self, year: i32, symbol: &str) -> Option<Option<&Dec>> { self.values.get(&(year, symbol.to_string())).map(Option::as_ref) }
}

/// ictax over the network, at `base` (ICTAX_BASE; a mock server in tests).
pub struct Live { pub base: String }

impl Live {
    /// The Kursliste of `day` for `symbols`: a symbol ictax does not list is `None`; a listed one with no value in the
    /// answer is left out (asked again next time). Any failure answers nothing.
    pub async fn values(&self, day: NaiveDate, symbols: &[String]) -> Result<Vec<(String, Option<Dec>)>, String> {
        let client = reqwest::Client::builder().connect_timeout(Duration::from_secs(10)).timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none()).build().map_err(|e| e.to_string())?;
        let session = client.get(format!("{}/authentication/session.json", self.base)).send().await.map_err(|e| e.to_string())?;
        // The cookies the session sets (JSESSIONID and the load balancer's), each `name=value`.
        let cookie = session.headers().get_all(reqwest::header::SET_COOKIE).iter().filter_map(|v| v.to_str().ok())
            .filter_map(|v| v.split(';').next()).map(str::trim).filter(|v| !v.is_empty()).collect::<Vec<_>>().join("; ");
        let session = answer(session).await?;
        let token = session["data"]["csrfToken"].as_str().filter(|t| !t.is_empty()).ok_or("no CSRF token in the session")?.to_string();
        let listed = answer(client.get(format!("{}/coreGadget/tokens.json", self.base)).header(reqwest::header::COOKIE, &cookie).send().await.map_err(|e| e.to_string())?).await?;
        // Branch review: a NULL is stored only from a whole catalogue: an entry with no symbol, or none at all, answers nothing.
        let entries = listed["data"].as_array().filter(|a| !a.is_empty()).ok_or("no token list")?;
        let listed: HashSet<&str> = entries.iter().map(|t| t["token"]["shortName"].as_str().filter(|s| !s.is_empty())).collect::<Option<_>>().ok_or("a token with no symbol")?;
        let mut out: Vec<(String, Option<Dec>)> = symbols.iter().filter(|s| !listed.contains(s.as_str())).map(|s| (s.clone(), None)).collect();
        let asked: Vec<&String> = symbols.iter().filter(|s| listed.contains(s.as_str())).collect();
        if asked.is_empty() { return Ok(out); }
        let midnight = day.and_hms_opt(0, 0, 0).ok_or("a day out of range")?.and_utc().timestamp_millis();
        let values = answer(client.post(format!("{}/coreGadget/tokenGadget.json", self.base)).header(reqwest::header::CONTENT_TYPE, "application/json")
            .header("X-CSRF-TOKEN", &token).header(reqwest::header::COOKIE, &cookie)
            .body(json!({ "referenceDate": midnight, "tokens": asked }).to_string()).send().await.map_err(|e| e.to_string())?).await?;
        for t in values["data"]["tokens"].as_array().ok_or("no token values")? {
            let Some(symbol) = t["token"]["shortName"].as_str().filter(|s| asked.iter().any(|a| a == s)) else { continue };
            let (Ok(value), Ok(denomination)) = (Dec::venue_number(&t["value"]), Dec::venue_number(&t["denomination"])) else { continue };
            if !value.is_positive() || !denomination.is_positive() { continue; }
            out.push((symbol.to_string(), Some(value.div(&denomination).map_err(|e| format!("{e:?}"))?)));
        }
        Ok(out)
    }
}

async fn answer(r: reqwest::Response) -> Result<Value, String> {
    let status = r.status().as_u16();
    if !(200..300).contains(&status) { return Err(format!("HTTP {status}")); }
    // Branch review: read chunk by chunk, never past 4 MiB (the catalogue is a few kB).
    let body = crate::venue::http::read_limited(r, 4 << 20).await.map_err(|e| format!("{e:?}"))?.body;
    let body: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    if body["status"].as_str() != Some("SUCCESS") { return Err(format!("status {}", body["status"])); }
    Ok(body)
}

/// Before a Swiss report: for each (year, symbols) of a finished year (1 January of the next has come), the symbols with no
/// row are asked of the Kursliste of 31 December and stored, in one transaction per year. A failure is logged and stores
/// nothing (the report values those coins at the market, provisional).
pub async fn ensure_loaded(db: &Db, live: &Live, needs: Vec<(i32, Vec<String>)>, today: NaiveDate) {
    for (year, symbols) in needs {
        let Some(day) = NaiveDate::from_ymd_opt(year, 12, 31).filter(|_| today.year() > year) else { continue };
        let stored = match db.run(|c, _| Estv::load(c).map_err(|e| format!("{e:?}"))).await { Ok(e) => e, Err(e) => { crate::engine::log(&format!("[TaxReport] Failed to read ESTV values: {e}")); return } };
        let missing: Vec<String> = symbols.into_iter().filter(|s| stored.value(year, s).is_none()).collect();
        if missing.is_empty() { continue; }
        let values = match live.values(day, &missing).await {
            Ok(v) => v,
            Err(e) => { crate::engine::log(&format!("[TaxReport] Failed to load the ESTV values of {year}: {e}")); continue }
        };
        let saved = db.run(move |c, _| {
            let tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
            for (symbol, value) in &values {
                tx.execute("INSERT INTO estv_crypto_values (year, symbol, value) VALUES (?1, ?2, ?3) ON CONFLICT (year, symbol) DO UPDATE SET value = excluded.value",
                           rusqlite::params![year, symbol, value.as_ref().map(Dec::to_s_f)]).map_err(|e| e.to_string())?;
            }
            tx.commit().map_err(|e| e.to_string())
        }).await;
        if let Err(e) = saved { crate::engine::log(&format!("[TaxReport] Failed to store the ESTV values of {year}: {e}")); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn db() -> Db {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../../migrations/20261106000001_estv_crypto_values.sql")).unwrap();
        Db::new(c, crate::crypto::Cipher::new(&crate::crypto::EncryptionKeys::resolve(&|_| None, "estv").unwrap()))
    }

    async fn rows(db: &Db) -> Vec<(i64, String, Option<String>)> {
        db.run(|c, _| {
            let mut s = c.prepare("SELECT year, symbol, value FROM estv_crypto_values ORDER BY year, symbol").unwrap();
            Ok(s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().collect::<Result<Vec<_>, _>>().unwrap())
        }).await.unwrap()
    }

    fn ok(data: Value) -> ResponseTemplate { ResponseTemplate::new(200).set_body_json(json!({ "status": "SUCCESS", "error": null, "data": data })) }

    async fn session(server: &MockServer) {
        Mock::given(method("GET")).and(path("/authentication/session.json"))
            .respond_with(ok(json!({ "csrfToken": "tok-1", "authenticated": false }))
                .append_header("Set-Cookie", "JSESSIONID=abc; Path=/extern; Secure; HttpOnly").append_header("Set-Cookie", "lb=xyz; path=/; HttpOnly"))
            .mount(server).await;
        Mock::given(method("GET")).and(path("/coreGadget/tokens.json")).and(header("Cookie", "JSESSIONID=abc; lb=xyz"))
            .respond_with(ok(json!([{ "token": { "shortName": "BTC" } }, { "token": { "shortName": "ETH" } }, { "token": { "shortName": "USDT" } }])))
            .mount(server).await;
    }

    fn needs() -> Vec<(i32, Vec<String>)> { vec![(2025, vec!["BTC".into(), "ETH".into(), "SOL".into(), "USDT".into()])] }
    fn day(s: &str) -> NaiveDate { NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap() }

    /// The session's CSRF token and cookies reach the POST; a listed token is stored per unit (ETH quoted per 10), one ictax
    /// does not list is stored NULL, a listed one the answer leaves out is not stored; a stored year is not asked again.
    #[tokio::test]
    async fn listed_and_unlisted_through_the_session_handshake() {
        let server = MockServer::start().await;
        session(&server).await;
        Mock::given(method("POST")).and(path("/coreGadget/tokenGadget.json")).and(header("X-CSRF-TOKEN", "tok-1")).and(header("Cookie", "JSESSIONID=abc; lb=xyz"))
            .and(body_json(json!({ "referenceDate": 1_767_139_200_000_i64, "tokens": ["BTC", "ETH", "USDT"] })))
            .respond_with(ok(json!({ "tokens": [{ "token": { "shortName": "BTC" }, "value": 69571.988489, "denomination": 1 },
                                                { "token": { "shortName": "ETH" }, "value": 24000, "denomination": 10 }] })))
            .expect(1).mount(&server).await;
        let db = db();
        let live = Live { base: server.uri() };
        ensure_loaded(&db, &live, needs(), day("2026-11-02")).await;
        assert_eq!(rows(&db).await, vec![(2025, "BTC".into(), Some("69571.988489".into())), (2025, "ETH".into(), Some("2400.0".into())), (2025, "SOL".into(), None)]);
        // Only USDT is asked again: the stored rows (a NULL included) are answers.
        Mock::given(method("POST")).and(path("/coreGadget/tokenGadget.json")).and(body_json(json!({ "referenceDate": 1_767_139_200_000_i64, "tokens": ["USDT"] })))
            .respond_with(ok(json!({ "tokens": [{ "token": { "shortName": "USDT" }, "value": 0.791468, "denomination": 1 }] }))).expect(1).mount(&server).await;
        ensure_loaded(&db, &live, needs(), day("2026-11-02")).await;
        let estv = db.run(|c, _| Estv::load(c).map_err(|e| format!("{e:?}"))).await.unwrap();
        assert_eq!(estv.value(2025, "USDT").map(|v| v.map(Dec::to_s_f)), Some(Some("0.791468".into())));
        assert_eq!(estv.value(2025, "SOL").map(|v| v.map(Dec::to_s_f)), Some(None));
        assert_eq!(estv.value(2024, "BTC"), None);
    }

    /// A failed session stores nothing, and a year not ended asks nothing.
    #[tokio::test]
    async fn a_failure_or_a_year_not_ended_stores_nothing() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/authentication/session.json")).respond_with(ResponseTemplate::new(503)).expect(1).mount(&server).await;
        let db = db();
        let live = Live { base: server.uri() };
        ensure_loaded(&db, &live, needs(), day("2026-11-02")).await;
        ensure_loaded(&db, &live, vec![(2026, vec!["BTC".into()])], day("2026-11-02")).await;
        assert_eq!(rows(&db).await, vec![]);
    }

    /// Branch review: a catalogue with an entry that names no symbol stores no NULL (nothing is "not listed" from it), and a
    /// body past the read limit is refused.
    #[tokio::test]
    async fn a_malformed_catalogue_or_an_oversized_body_stores_nothing() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/authentication/session.json")).respond_with(ok(json!({ "csrfToken": "tok-1" }))).mount(&server).await;
        Mock::given(method("GET")).and(path("/coreGadget/tokens.json")).respond_with(ok(json!([{ "token": {} }]))).expect(1).mount(&server).await;
        let db = db();
        let live = Live { base: server.uri() };
        ensure_loaded(&db, &live, needs(), day("2026-11-02")).await;
        assert_eq!(rows(&db).await, vec![]);
        let big = MockServer::start().await;
        Mock::given(method("GET")).and(path("/authentication/session.json"))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!("{{\"status\":\"SUCCESS\",\"pad\":\"{}\"}}", "x".repeat(5 << 20)))).mount(&big).await;
        assert!(Live { base: big.uri() }.values(day("2025-12-31"), &["BTC".into()]).await.is_err());
    }

}
