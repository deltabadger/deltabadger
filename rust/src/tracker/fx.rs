//! Tax::EcbFxRates (app/models/tax/ecb_fx_rates.rb): the ECB's daily euro reference rates in `fx_rates`, read with a
//! week's lookback and crossed through the euro, and `ensure_loaded!`, which tops the table up from the ECB (the
//! Bundesbank's copy of the same series as the fallback) whenever its newest dollar rate is older than the last weekday
//! before today. Every place Rails builds a Tax::PriceService, or calls `ensure_loaded!` itself, loads here too.
use crate::figures::{dec::Dec, FiguresError};
use crate::jobs::Db;
use crate::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use chrono::{Datelike, Days, NaiveDate, Weekday};
use rusqlite::Connection;
use std::collections::{BTreeMap, HashMap};

/// Tax::EcbFxRates::CURRENCIES.
pub const CURRENCIES: [&str; 11] = ["USD", "GBP", "CHF", "SEK", "DKK", "CZK", "PLN", "NOK", "HUF", "RON", "BGN"];
/// Tax::EcbFxRates::LOOKBACK_DAYS.
pub const LOOKBACK_DAYS: u64 = 7;
pub const ECB_BASE: &str = "https://data-api.ecb.europa.eu";
pub const BUNDESBANK_BASE: &str = "https://api.statistiken.bundesbank.de";
/// The largest answer read: the whole 1999-onwards history of eleven currencies is about 3 MB.
const MAX_BODY: usize = 16 * 1024 * 1024;

pub fn ecb_path() -> String { format!("/service/data/EXR/D.{}.EUR.SP00.A", CURRENCIES.join("+")) }
pub fn bundesbank_path(currency: &str) -> String { format!("/rest/download/BBEX3/D.{currency}.EUR.BB.AC.000") }

/// `fx_rates`, by currency and day, as ActiveRecord reads the decimal column.
#[derive(Clone, Debug, Default)]
pub struct FxTable { rates: HashMap<String, BTreeMap<NaiveDate, Dec>> }

impl FxTable {
    /// The whole table, each row a step of the open budget scope.
    pub fn load(c: &Connection) -> Result<FxTable, FiguresError> {
        let mut t = FxTable::default();
        let mut s = c.prepare("SELECT currency, date, rate FROM fx_rates")?;
        let mut q = s.query([])?;
        while let Some(r) = q.next()? {
            crate::figures::budget::charge(1, 0)?;
            let (currency, date): (String, String) = (r.get(0)?, r.get(1)?);
            let (Ok(date), Some(rate)) = (NaiveDate::parse_from_str(&date, "%Y-%m-%d"), Dec::from_sql(r.get_ref(2)?)?) else { continue };
            t.rates.entry(currency).or_default().insert(date, rate);
        }
        Ok(t)
    }

    /// `per_eur`: the newest rate of the week up to `date`.
    fn per_eur(&self, currency: &str, date: NaiveDate) -> Option<&Dec> {
        let from = date.checked_sub_days(Days::new(LOOKBACK_DAYS)).unwrap_or(date);
        self.rates.get(currency)?.range(from..=date).next_back().map(|(_, r)| r)
    }

    /// `rate(from:, to:, date:)`: units of `to` per unit of `from`. None is Rails' MissingRate.
    pub fn rate(&self, from: &str, to: &str, date: NaiveDate) -> Result<Option<Dec>, FiguresError> {
        if from == to { return Ok(Some(Dec::one())); }
        let one = Dec::one();
        let from_rate = if from == "EUR" { Some(&one) } else { self.per_eur(from, date) };
        let to_rate = if to == "EUR" { Some(&one) } else { self.per_eur(to, date) };
        match (from_rate, to_rate) {
            (Some(f), Some(t)) => Ok(Some(t.div(f)?)),
            _ => Ok(None),
        }
    }

    /// The newest stored dollar rate's day.
    pub fn newest_usd(&self) -> Option<NaiveDate> { self.rates.get("USD").and_then(|d| d.keys().next_back().copied()) }
}

/// `last_expected_publication_date`: yesterday, walked back over a weekend.
pub fn last_expected(today: NaiveDate) -> NaiveDate {
    let mut d = today.pred_opt().unwrap_or(today);
    while matches!(d.weekday(), Weekday::Sat | Weekday::Sun) { d = d.pred_opt().unwrap_or(d); }
    d
}

/// A plain GET over the network, as Rails' bare `Faraday.new` sends it (15 s timeouts, no headers of ours).
#[derive(Clone)]
pub struct Plain { client: reqwest::Client }

impl Default for Plain {
    fn default() -> Self {
        let t = std::time::Duration::from_secs(15);
        Plain { client: crate::venue::http::client_with(t, t, std::time::Duration::from_secs(30)) }
    }
}

impl Transport for Plain {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> { self.send_limited(r, MAX_BODY).await }
    async fn send_limited(&self, r: &HttpRequest, limit: usize) -> Result<HttpResponse, TransportError> {
        let resp = self.client.get(r.url()).send().await.map_err(|e| TransportError::NotSent(e.to_string()))?;
        // Read chunk by chunk, never past `limit`.
        crate::venue::http::read_limited(resp, limit).await
    }
}

/// Where the rates are fetched from: the network, or a script (tests and the parity harness).
#[derive(Clone)]
pub enum Source { Live(Plain), Scripted(crate::venue::http::ScriptedTransport) }

impl Source {
    async fn get(&self, base: &str, path: String, query: Vec<(&'static str, String)>) -> Result<HttpResponse, TransportError> {
        let req = HttpRequest { method: "GET", base: base.into(), path, query, body: None, not_after: None };
        match self { Source::Live(t) => t.send_limited(&req, MAX_BODY).await, Source::Scripted(t) => t.send_limited(&req, MAX_BODY).await }
    }
}

/// Ruby's CSV.parse, for the bodies these sources send: fields split on commas, a quoted field may hold commas, newlines
/// and doubled quotes. A quote anywhere else is malformed, as Ruby raises MalformedCSVError.
pub fn csv(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut rows = vec![];
    let (mut row, mut field, mut quoted, mut was_quoted) = (vec![], String::new(), false, false);
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') { chars.next(); field.push('"'); } else { quoted = false; }
            } else { field.push(ch); }
            continue;
        }
        match ch {
            '"' if field.is_empty() && !was_quoted => { quoted = true; was_quoted = true; }
            '"' => return Err("Illegal quoting".into()),
            ',' => { row.push(std::mem::take(&mut field)); was_quoted = false; }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' => { row.push(std::mem::take(&mut field)); rows.push(std::mem::take(&mut row)); was_quoted = false; }
            c if was_quoted => return Err(format!("Any value after quoted field isn't allowed ({c})")),
            c => field.push(c),
        }
    }
    if quoted { return Err("Unclosed quoted field".into()); }
    if !field.is_empty() || !row.is_empty() || was_quoted { row.push(field); rows.push(row); }
    Ok(rows)
}

/// `bundesbank_to_sdmx`: the BOM gone, then the rows whose first field is a day and second a plain decimal.
fn bundesbank_rows(body: &str, currency: &str) -> Result<Vec<(String, String, String)>, String> {
    let body = body.strip_prefix('\u{feff}').unwrap_or(body);
    let day = regex::Regex::new(r"\A\d{4}-\d{2}-\d{2}\z").map_err(|e| e.to_string())?;
    let value = regex::Regex::new(r"\A\d+(\.\d+)?\z").map_err(|e| e.to_string())?;
    Ok(csv(body)?.into_iter().filter_map(|row| {
        let (d, v) = (row.first()?, row.get(1)?);
        (day.is_match(d) && value.is_match(v)).then(|| (currency.to_string(), d.clone(), v.clone()))
    }).collect())
}

/// What `ensure_loaded!` parses out of the SDMX body: (currency, day, value) of every row with a value.
fn sdmx_rows(body: &str) -> Result<Vec<(String, String, String)>, String> {
    let mut rows = csv(body)?.into_iter();
    let header = rows.next().unwrap_or_default();
    let col = |name: &str| header.iter().position(|h| h == name);
    let (Some(cur), Some(day), Some(val)) = (col("CURRENCY"), col("TIME_PERIOD"), col("OBS_VALUE")) else { return Ok(vec![]) };
    Ok(rows.filter_map(|r| {
        let v = r.get(val).cloned().unwrap_or_default();
        if crate::ruby::blank(&v) { return None; }
        Some((r.get(cur).cloned().unwrap_or_default(), r.get(day).cloned().unwrap_or_default(), v))
    }).collect())
}

/// `fetch_history_csv`: the ECB's answer when it succeeded and holds observations, else the Bundesbank's, one request a
/// currency (a transport failure there fails the load, as Rails raises it into `ensure_loaded!`'s rescue).
async fn fetch_rows(source: &Source) -> Result<Vec<(String, String, String)>, String> {
    let ecb = source.get(ECB_BASE, ecb_path(), vec![("format", "csvdata".into()), ("startPeriod", "1999-01-01".into())]).await;
    if let Ok(r) = &ecb { if (200..300).contains(&r.status) && r.body.contains("OBS_VALUE") { return sdmx_rows(&r.body); } }
    let mut rows = vec![];
    for currency in CURRENCIES {
        let r = source.get(BUNDESBANK_BASE, bundesbank_path(currency), vec![("format", "csv".into()), ("lang", "en".into())]).await
            .map_err(|e| format!("{e:?}"))?;
        rows.extend(bundesbank_rows(&r.body, currency)?);
    }
    Ok(rows)
}

/// `value.to_d` on a CSV field, as Ruby's String#to_d reads it: the leading number, else zero.
fn to_d(v: &str) -> Dec {
    let n: String = v.trim_start().chars().enumerate().take_while(|(i, c)| c.is_ascii_digit() || *c == '.' || (*i == 0 && (*c == '-' || *c == '+'))).map(|(_, c)| c).collect();
    Dec::strict(n.trim_end_matches('.')).unwrap_or_else(|_| Dec::zero())
}

/// `ensure_loaded!`: nothing when the newest dollar rate is current; else the history fetched and upserted by (currency,
/// day), 5,000 rows a unit. Any failure is logged and the caller goes on, as Rails rescues it.
pub async fn ensure_loaded(db: &Db, source: Option<&Source>, today: NaiveDate) {
    let Some(source) = source else { return };
    let newest = db.run(|c, _| c.query_row("SELECT max(date) FROM fx_rates WHERE currency = 'USD'", [], |r| r.get::<_, Option<String>>(0)).map_err(|e| e.to_string())).await;
    let newest = match newest { Ok(n) => n.and_then(|d| NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok()), Err(e) => { crate::engine::log(&format!("[TaxReport] Failed to load ECB FX rates: {e}")); return } };
    if newest.is_some_and(|n| n >= last_expected(today)) { return; }
    let rows = match fetch_rows(source).await {
        Ok(rows) => rows,
        Err(e) => { crate::engine::log(&format!("[TaxReport] Failed to load ECB FX rates: {e}")); return }
    };
    let mut parsed = vec![];
    for (currency, day, value) in rows {
        let Ok(day) = NaiveDate::parse_from_str(day.trim(), "%Y-%m-%d") else { crate::engine::log("[TaxReport] Failed to load ECB FX rates: Date::Error: invalid date"); return };
        parsed.push((currency, day.to_string(), to_d(&value).to_s_f()));
    }
    let stored = db.run(move |c, _| {
        for chunk in parsed.chunks(5_000) {
            let tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
            {
                let mut s = tx.prepare_cached("INSERT INTO fx_rates (currency, date, rate) VALUES (?1, ?2, ?3) ON CONFLICT (currency, date) DO UPDATE SET rate = excluded.rate")
                    .map_err(|e| e.to_string())?;
                for (currency, day, rate) in chunk { s.execute(rusqlite::params![currency, day, rate]).map_err(|e| e.to_string())?; }
            }
            tx.commit().map_err(|e| e.to_string())?;
        }
        Ok(())
    }).await;
    if let Err(e) = stored { crate::engine::log(&format!("[TaxReport] Failed to load ECB FX rates: {e}")); }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> NaiveDate { NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap() }

    #[test]
    fn rates_cross_through_the_euro_with_a_week_of_lookback() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE fx_rates (currency TEXT, date TEXT, rate NUMERIC, UNIQUE (currency, date));
                         INSERT INTO fx_rates VALUES ('USD', '2026-09-25', '1.17'), ('GBP', '2026-09-25', '0.9'), ('USD', '2026-09-30', '1.2');").unwrap();
        let t = FxTable::load(&c).unwrap();
        assert_eq!(t.rate("EUR", "USD", day("2026-09-30")).unwrap().unwrap().to_s_f(), "1.2");
        assert_eq!(t.rate("EUR", "USD", day("2026-09-29")).unwrap().unwrap().to_s_f(), "1.17");
        assert_eq!(t.rate("GBP", "USD", day("2026-09-26")).unwrap().unwrap().to_s_f(), "1.3");
        assert!(t.rate("GBP", "USD", day("2026-10-03")).unwrap().is_none(), "eight days after the last GBP rate");
        assert_eq!(t.rate("USD", "USD", day("1990-01-01")).unwrap().unwrap().to_s_f(), "1.0");
    }

    #[test]
    fn the_publication_day_skips_the_weekend() {
        assert_eq!(last_expected(day("2026-10-13")), day("2026-10-12")); // Tuesday
        assert_eq!(last_expected(day("2026-10-12")), day("2026-10-09")); // Monday
        assert_eq!(last_expected(day("2026-10-11")), day("2026-10-09")); // Sunday
    }

    /// A chunked answer is read chunk by chunk and refused once it passes the limit, never buffered whole first.
    #[tokio::test(flavor = "current_thread")]
    async fn an_oversized_chunked_answer_is_refused_while_it_is_read() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request).await;
            socket.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
            let chunk = vec![b'x'; 1024];
            for _ in 0..64 {
                if socket.write_all(format!("{:x}\r\n", chunk.len()).as_bytes()).await.is_err() { return; }
                if socket.write_all(&chunk).await.is_err() || socket.write_all(b"\r\n").await.is_err() { return; }
            }
            let _ = socket.write_all(b"0\r\n\r\n").await;
        });
        let req = HttpRequest { method: "GET", base: format!("http://{addr}"), path: "/big".into(), query: vec![], body: None, not_after: None };
        let out = Plain::default().send_limited(&req, 8 * 1024).await;
        assert!(matches!(out, Err(TransportError::MaybeSent(ref m)) if m.contains("over 8192 bytes")), "{out:?}");
        server.abort();
    }

    #[test]
    fn csv_reads_quotes_and_refuses_stray_ones() {
        assert_eq!(csv("a,\"b,c\",\"d\"\"e\"\r\n1,2,3\n").unwrap(), vec![vec!["a", "b,c", "d\"e"], vec!["1", "2", "3"]]);
        assert!(csv("a,b\"c\n").is_err());
        let rows = bundesbank_rows("\u{feff}\"\",\"BBEX3\"\n2026-09-30,1.1700,\n2026-10-01,.,\n", "USD").unwrap();
        assert_eq!(rows, vec![("USD".to_string(), "2026-09-30".to_string(), "1.1700".to_string())]);
    }
}
