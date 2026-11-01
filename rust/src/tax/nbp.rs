//! D-TAX-PL-NBP: the NBP's table A mid rates Poland's report converts at (art. 11a PIT act: the mid rate of the last
//! business day before the day of the revenue or cost), in `nbp_rates`, and the ranges fetched in full, in `nbp_coverage`.
//! "The last table before the day" is read only across covered days: a covered day with no table is a day NBP published
//! none (a holiday); a day outside coverage is unknown, so the value is missing even when an older table is stored.
//! Rust-only (Rails converts at the ECB's rate); the job fills the tables before the report, as `fx::ensure_loaded` does.
use crate::figures::{dec::Dec, FiguresError};
use crate::jobs::Db;
use crate::tracker::fx::Source;
use chrono::{Days, NaiveDate};
use rusqlite::Connection;
use std::collections::{BTreeMap, HashMap};

pub const NBP_BASE: &str = "https://api.nbp.pl";
/// api.nbp.pl answers at most 93 days a query.
const MAX_DAYS: u64 = 93;
/// From a week before the first value: the table that precedes it.
const LEAD_DAYS: u64 = 7;

pub fn path(currency: &str, from: NaiveDate, to: NaiveDate) -> String {
    format!("/api/exchangerates/rates/a/{}/{from}/{to}/", currency.to_lowercase())
}

/// Both tables, by currency.
#[derive(Clone, Debug, Default)]
pub struct Nbp { rates: HashMap<String, BTreeMap<NaiveDate, Dec>>, coverage: HashMap<String, Vec<(NaiveDate, NaiveDate)>> }

fn date(s: &str) -> Option<NaiveDate> { NaiveDate::parse_from_str(s, "%Y-%m-%d").ok() }

impl Nbp {
    pub fn load(c: &Connection) -> Result<Nbp, FiguresError> {
        let mut t = Nbp::default();
        let mut s = c.prepare("SELECT currency, date, rate FROM nbp_rates")?;
        let mut q = s.query([])?;
        while let Some(r) = q.next()? {
            crate::figures::budget::charge(1, 0)?;
            let (currency, day): (String, String) = (r.get(0)?, r.get(1)?);
            let (Some(day), Some(rate)) = (date(&day), Dec::from_sql(r.get_ref(2)?)?) else { continue };
            t.rates.entry(currency).or_default().insert(day, rate);
        }
        t.coverage = coverage(c)?;
        Ok(t)
    }

    fn covered(&self, currency: &str, day: NaiveDate) -> bool {
        self.coverage.get(currency).is_some_and(|ranges| ranges.iter().any(|(f, t)| *f <= day && day <= *t))
    }

    /// The last table published before `day`, read back across covered days only: (its date, its rate).
    pub fn before(&self, currency: &str, day: NaiveDate) -> Option<(NaiveDate, &Dec)> {
        let mut d = day.pred_opt()?;
        while self.covered(currency, d) {
            if let Some(rate) = self.rates.get(currency).and_then(|r| r.get(&d)) { return Some((d, rate)); }
            d = d.pred_opt()?;
        }
        None
    }
}

fn coverage(c: &Connection) -> rusqlite::Result<HashMap<String, Vec<(NaiveDate, NaiveDate)>>> {
    let mut out: HashMap<String, Vec<(NaiveDate, NaiveDate)>> = HashMap::new();
    let mut s = c.prepare("SELECT currency, from_date, to_date FROM nbp_coverage")?;
    for row in s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))? {
        let (currency, from, to) = row?;
        if let (Some(f), Some(t)) = (date(&from), date(&to)) { out.entry(currency).or_default().push((f, t)); }
    }
    Ok(out)
}

/// The days of [from, to] no range covers, as ranges of at most 93 days.
fn gaps(covered: &[(NaiveDate, NaiveDate)], from: NaiveDate, to: NaiveDate) -> Vec<(NaiveDate, NaiveDate)> {
    let mut out: Vec<(NaiveDate, NaiveDate)> = vec![];
    let mut d = from;
    while d <= to {
        if !covered.iter().any(|(f, t)| *f <= d && d <= *t) {
            match out.last_mut() {
                Some((start, end)) if end.succ_opt() == Some(d) && (d - *start).num_days() < MAX_DAYS as i64 => *end = d,
                _ => out.push((d, d)),
            }
        }
        let Some(next) = d.succ_opt() else { break };
        d = next;
    }
    out
}

/// Table A of `currency` over [from, to], when NBP answered for the whole range: its (date, mid) rows; a 404 is NBP's
/// "Brak danych", no table in the range (a currency table A lacks is the same: never a rate).
async fn fetch(source: &Source, currency: &str, from: NaiveDate, to: NaiveDate) -> Result<Vec<(String, String)>, String> {
    let r = source.get(NBP_BASE, path(currency, from, to), vec![("format", "json".into())]).await.map_err(|e| format!("{e:?}"))?;
    if r.status == 404 { return Ok(vec![]); }
    if !(200..300).contains(&r.status) { return Err(format!("HTTP {}", r.status)); }
    let body: serde_json::Value = serde_json::from_str(&r.body).map_err(|e| e.to_string())?;
    let rates = body["rates"].as_array().ok_or("no rates")?;
    rates.iter().map(|x| {
        let day = x["effectiveDate"].as_str().and_then(date).filter(|d| (from..=to).contains(d)).ok_or("a rate outside the range asked")?;
        let mid = x["mid"].as_number().map(ToString::to_string).ok_or("a rate with no mid")?;
        Dec::strict(&mid).ok().filter(Dec::is_positive).ok_or("a mid that is not a positive number")?;
        Ok((day.to_string(), mid))
    }).collect()
}

/// Before Poland's report: for each (currency, day of its first value), NBP's tables from a week before that day to
/// yesterday (today's may not be out yet), the ranges no coverage holds fetched, each stored with its coverage in one
/// transaction only when it came back whole. A failure is logged and the range stays uncovered: the report lists every
/// value it then lacks.
pub async fn ensure_loaded(db: &Db, source: Option<&Source>, needs: Vec<(String, NaiveDate)>, today: NaiveDate) {
    let Some(source) = source else { return };
    if needs.is_empty() { return; }
    let covered = match db.run(|c, _| coverage(c).map_err(|e| e.to_string())).await {
        Ok(c) => c,
        Err(e) => { crate::engine::log(&format!("[TaxReport] Failed to load NBP rates: {e}")); return }
    };
    let Some(to) = today.pred_opt() else { return };
    for (currency, first) in needs {
        let from = first.checked_sub_days(Days::new(LEAD_DAYS)).unwrap_or(first);
        for (f, t) in gaps(covered.get(&currency).map_or(&[][..], Vec::as_slice), from, to) {
            let rows = match fetch(source, &currency, f, t).await {
                Ok(rows) => rows,
                Err(e) => { crate::engine::log(&format!("[TaxReport] Failed to load NBP {currency} rates {f}..{t}: {e}")); continue }
            };
            let cur = currency.clone();
            let stored = db.run(move |c, _| {
                let tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
                for (day, rate) in &rows {
                    tx.execute("INSERT INTO nbp_rates (currency, date, rate) VALUES (?1, ?2, ?3) ON CONFLICT (currency, date) DO UPDATE SET rate = excluded.rate",
                               rusqlite::params![cur, day, rate]).map_err(|e| e.to_string())?;
                }
                tx.execute("INSERT INTO nbp_coverage (currency, from_date, to_date) VALUES (?1, ?2, ?3)", rusqlite::params![cur, f.to_string(), t.to_string()])
                    .map_err(|e| e.to_string())?;
                tx.commit().map_err(|e| e.to_string())
            }).await;
            if let Err(e) = stored { crate::engine::log(&format!("[TaxReport] Failed to store NBP {currency} rates: {e}")); }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venue::http::ScriptedTransport;
    use serde_json::json;

    fn day(s: &str) -> NaiveDate { date(s).unwrap() }

    fn db() -> Db {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../../migrations/20261103000001_nbp_rates.sql")).unwrap();
        Db::new(c, crate::crypto::Cipher::new(&crate::crypto::EncryptionKeys::resolve(&|_| None, "nbp").unwrap()))
    }

    async fn table(db: &Db) -> Nbp { db.run(|c, _| Nbp::load(c).map_err(|e| format!("{e:?}"))).await.unwrap() }

    fn key(currency: &str, from: &str, to: &str) -> String { format!("GET {}?format=json", path(currency, day(from), day(to))) }

    /// A Saturday sale needs Friday's table. Wednesday's is stored and covered; the fetch of the days after it fails, so
    /// they stay uncovered and Saturday has no rate (Wednesday's is not used). The next run fetches them again: Friday's
    /// table serves Saturday and Monday, and a day NBP published nothing for (Thursday here) is covered, not a gap.
    #[tokio::test(flavor = "current_thread")]
    async fn a_failed_range_is_never_covered_and_the_refetch_fills_it() {
        let db = db();
        db.run(|c, _| c.execute_batch("INSERT INTO nbp_rates VALUES ('EUR', '2025-03-12', '4.18'); INSERT INTO nbp_coverage VALUES ('EUR', '2025-03-08', '2025-03-12');")
            .map_err(|e| e.to_string())).await.unwrap();
        let t = ScriptedTransport::default();
        t.reply(&key("EUR", "2025-03-13", "2025-03-16"), 500, json!({}));
        t.reply(&key("EUR", "2025-03-13", "2025-03-16"), 200, json!({ "table": "A", "code": "EUR", "rates": [
            { "no": "051/A/NBP/2025", "effectiveDate": "2025-03-14", "mid": 4.1790 }] }));
        let source = Source::Scripted(t.clone());
        let needs = vec![("EUR".to_string(), day("2025-03-15"))];
        ensure_loaded(&db, Some(&source), needs.clone(), day("2025-03-17")).await;
        let nbp = table(&db).await;
        assert_eq!(nbp.before("EUR", day("2025-03-13")).map(|(d, r)| (d, r.to_s_f())), Some((day("2025-03-12"), "4.18".into())));
        assert!(nbp.before("EUR", day("2025-03-15")).is_none(), "Friday is not covered: no rate, not Wednesday's");
        ensure_loaded(&db, Some(&source), needs.clone(), day("2025-03-17")).await;
        let nbp = table(&db).await;
        for sale in ["2025-03-15", "2025-03-17"] {
            assert_eq!(nbp.before("EUR", day(sale)).map(|(d, r)| (d, r.to_s_f())), Some((day("2025-03-14"), "4.179".into())), "{sale}");
        }
        assert_eq!(nbp.before("EUR", day("2025-03-14")).map(|(d, _)| d), Some(day("2025-03-12")), "Thursday had no table");
        // Covered now: a third run asks for nothing.
        ensure_loaded(&db, Some(&source), needs, day("2025-03-17")).await;
        assert_eq!(t.requests().len(), 2);
    }

    /// From a week before the first value to yesterday, in ranges of at most 93 days; NBP's 404 (no table in the range)
    /// covers its range.
    #[tokio::test(flavor = "current_thread")]
    async fn ranges_are_at_most_93_days_and_a_404_is_no_table() {
        let db = db();
        let t = ScriptedTransport::default();
        t.reply(&key("USD", "2025-01-01", "2025-04-03"), 200, json!({ "rates": [{ "effectiveDate": "2025-01-02", "mid": 4.1012 }] }));
        t.reply(&key("USD", "2025-04-04", "2025-05-31"), 404, json!("404 NotFound - Not Found - Brak danych"));
        ensure_loaded(&db, Some(&Source::Scripted(t.clone())), vec![("USD".to_string(), day("2025-01-08"))], day("2025-06-01")).await;
        assert_eq!(t.requests().len(), 2);
        let nbp = table(&db).await;
        assert_eq!(nbp.before("USD", day("2025-05-31")).map(|(d, r)| (d, r.to_s_f())), Some((day("2025-01-02"), "4.1012".into())));
        assert!(nbp.before("USD", day("2025-01-02")).is_none(), "1 January is covered but nothing before it is");
    }

    #[test]
    fn gaps_skip_what_is_covered() {
        let covered = [(day("2025-01-05"), day("2025-01-10"))];
        assert_eq!(gaps(&covered, day("2025-01-01"), day("2025-01-12")), vec![(day("2025-01-01"), day("2025-01-04")), (day("2025-01-11"), day("2025-01-12"))]);
        assert_eq!(gaps(&[], day("2025-01-01"), day("2025-04-04")).len(), 2);
        assert_eq!(gaps(&[], day("2025-01-01"), day("2025-04-03")), vec![(day("2025-01-01"), day("2025-04-03"))]);
    }
}
