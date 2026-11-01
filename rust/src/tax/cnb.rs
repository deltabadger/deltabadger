//! D-TAX-CZ-CNB: the ČNB's daily fixing the Czech report converts at (ZDP § 38 odst. 1 písm. b) bod 1, odst. 5: the rate
//! for the day of the income or expense), in `cnb_rates` (CZK per one unit), and the days the fetched year files prove,
//! in `cnb_coverage`. A fixing holds for its business day and the following non-business days (§ 35 zákona o ČNB), so a
//! day's rate is the latest fixing dated on or before it, read back across covered days only: outside coverage a value
//! is missing (banner, listed, 0), never an older stored fixing. Any currency's fixing defines the fixing day; a fixing
//! that omits the currency is missing for it. Rust-only (Rails converts at the ECB's rate); the job fills the tables before
//! the report, as `nbp::ensure_loaded` does.
use crate::figures::{at::At, dec::Dec, FiguresError};
use crate::jobs::Db;
use crate::tracker::fx::Source;
use crate::tracker::prices::Statutory;
use chrono::{Datelike, NaiveDate};
use rusqlite::Connection;
use std::collections::{BTreeMap, HashMap};

pub const CNB_BASE: &str = "https://www.cnb.cz";
const FIXING: &str = "/en/financial-markets/foreign-exchange-market/central-bank-exchange-rate-fixing/central-bank-exchange-rate-fixing";
pub fn year_path() -> String { format!("{FIXING}/year.txt") }
pub fn daily_path() -> String { format!("{FIXING}/daily.txt") }

/// Both tables: the fixings by day, and the covered days.
#[derive(Clone, Debug, Default)]
pub struct Cnb { fixings: BTreeMap<NaiveDate, HashMap<String, Dec>>, coverage: Vec<(NaiveDate, NaiveDate)> }

fn date(s: &str) -> Option<NaiveDate> { NaiveDate::parse_from_str(s, "%Y-%m-%d").ok() }

impl Cnb {
    pub fn load(c: &Connection) -> Result<Cnb, FiguresError> {
        let mut t = Cnb::default();
        let mut s = c.prepare("SELECT currency, date, rate FROM cnb_rates")?;
        let mut q = s.query([])?;
        while let Some(r) = q.next()? {
            crate::figures::budget::charge(1, 0)?;
            let (currency, day): (String, String) = (r.get(0)?, r.get(1)?);
            let (Some(day), Some(rate)) = (date(&day), Dec::from_sql(r.get_ref(2)?)?) else { continue };
            t.fixings.entry(day).or_default().insert(currency, rate);
        }
        t.coverage = coverage(c)?;
        Ok(t)
    }

    fn covered(&self, day: NaiveDate) -> bool { self.coverage.iter().any(|(f, t)| *f <= day && day <= *t) }

    /// CZK per unit of `currency` on `day`: the latest fixing on or before it within coverage.
    pub fn rate(&self, currency: &str, day: NaiveDate) -> Option<&Dec> {
        let mut d = day;
        while self.covered(d) {
            if let Some(fixing) = self.fixings.get(&d) { return fixing.get(currency); }
            d = d.pred_opt()?;
        }
        None
    }
}

/// The ČNB conversion of a Czech report: on the taxpayer's local day; CZK is the fixings' own unit.
pub struct Daily<'a> { pub cnb: &'a Cnb, pub zone: &'a str }

impl Statutory for Daily<'_> {
    fn day(&self, at: At) -> NaiveDate { crate::web::timezone::local(at.utc(), self.zone).date_naive() }
    fn rate(&self, from: &str, to: &str, day: NaiveDate) -> Option<Dec> {
        let czk = |c: &str| if c == "CZK" { Some(Dec::one()) } else { self.cnb.rate(c, day).cloned() };
        if from == to { return Some(Dec::one()); }
        czk(from)?.div(&czk(to)?).ok()
    }
}

fn coverage(c: &Connection) -> rusqlite::Result<Vec<(NaiveDate, NaiveDate)>> {
    let mut s = c.prepare("SELECT from_date, to_date FROM cnb_coverage")?;
    let rows = s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows.iter().filter_map(|(f, t)| Some((date(f)?, date(t)?))).collect())
}

/// One fixing: its day and its rates per unit, by currency.
type Fixing = (NaiveDate, Vec<(String, Dec)>);

/// A year file (`Date|1 AUD|…|100 JPY|…`, the header repeated where the currencies change, then `DD.MM.YYYY|rate|…`):
/// each fixing's rates per one unit. Refused unless every line parses, every row has its header's count, and the rows lie
/// in `year`.
fn parse_year(body: &str, year: i32) -> Result<Vec<Fixing>, String> {
    // A body cut short can still parse line by line (`…|21.368` cut to `…|2`): only a body ending with its line break,
    // every rate in ČNB's three decimals, is whole.
    if !body.ends_with('\n') { return Err("a body not ending with a line break".into()); }
    let mut header: Option<Vec<(Dec, String)>> = None;
    let mut out = vec![];
    for line in body.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let fields: Vec<&str> = line.split('|').collect();
        if fields[0] == "Date" {
            header = Some(fields[1..].iter().map(|h| {
                let (amount, code) = h.trim().split_once(' ').ok_or_else(|| format!("a header column {h:?}"))?;
                let amount = Dec::strict(amount).ok().filter(Dec::is_positive).ok_or_else(|| format!("a header amount {h:?}"))?;
                Ok((amount, code.trim().to_string()))
            }).collect::<Result<_, String>>()?);
            continue;
        }
        let columns = header.as_ref().ok_or("a row before any header")?;
        let day = NaiveDate::parse_from_str(fields[0], "%d.%m.%Y").map_err(|_| format!("a row date {:?}", fields[0]))?;
        if day.year() != year { return Err(format!("a row of {day} in the file of {year}")); }
        if fields.len() - 1 != columns.len() { return Err(format!("{} rates under {} columns on {day}", fields.len() - 1, columns.len())); }
        let rates = fields[1..].iter().zip(columns).map(|(v, (amount, code))| {
            let v = v.trim().replace(',', ".");
            let three = v.split_once('.').is_some_and(|(i, f)| !i.is_empty() && i.bytes().all(|b| b.is_ascii_digit()) && f.len() == 3 && f.bytes().all(|b| b.is_ascii_digit()));
            let v = Dec::strict(&v).ok().filter(|v| three && v.is_positive()).ok_or_else(|| format!("a rate {v:?} on {day}"))?;
            Ok((code.clone(), v.div(amount).map_err(|e| format!("{e:?}"))?))
        }).collect::<Result<Vec<_>, String>>()?;
        out.push((day, rates));
    }
    if out.is_empty() { return Err("no fixing".into()); }
    Ok(out)
}

/// The date `daily.txt` names on its first line (`31 Dec 2025 #250`, or `31.12.2025 #250`): the fixing valid that day.
fn daily_date(body: &str) -> Option<NaiveDate> {
    let first = body.lines().next()?.split('#').next()?.trim();
    NaiveDate::parse_from_str(first, "%d %b %Y").or_else(|_| NaiveDate::parse_from_str(first, "%d.%m.%Y")).ok()
}

async fn get(source: &Source, path: String, query: Vec<(&'static str, String)>) -> Result<String, String> {
    let r = source.get(CNB_BASE, path, query).await.map_err(|e| format!("{e:?}"))?;
    if !(200..300).contains(&r.status) { return Err(format!("HTTP {}", r.status)); }
    Ok(r.body)
}

/// Before a Czech report: one year file per year of `years` (from the year of the first value less a week to the report
/// year) whose days up to its target end (31 December; for the current year, yesterday) are not all covered. A file covers
/// 1 January to its last fixing L; the days after L up to the end are covered only when `daily.txt` for the end answers
/// with L's fixing (none was published after it; a later date means the file was cut short, and the year is fetched again
/// next time). Rates and coverage are written in one transaction; a failure is logged and leaves the year uncovered.
pub async fn ensure_loaded(db: &Db, source: Option<&Source>, years: Vec<i32>, today: NaiveDate) {
    let Some(source) = source else { return };
    let covered = match db.run(|c, _| coverage(c).map_err(|e| e.to_string())).await {
        Ok(c) => c,
        Err(e) => { crate::engine::log(&format!("[TaxReport] Failed to load ČNB rates: {e}")); return }
    };
    for year in years {
        let (Some(first), Some(end)) = (NaiveDate::from_ymd_opt(year, 1, 1), if year == today.year() { today.pred_opt() } else { NaiveDate::from_ymd_opt(year, 12, 31) }) else { continue };
        if year > today.year() || end < first { continue; }
        if first.iter_days().take_while(|d| *d <= end).all(|d| covered.iter().any(|(f, t)| *f <= d && d <= *t)) { continue; }
        let fixings = match get(source, year_path(), vec![("year", year.to_string())]).await.and_then(|body| parse_year(&body, year)) {
            Ok(f) => f,
            Err(e) => { crate::engine::log(&format!("[TaxReport] Failed to load ČNB rates of {year}: {e}")); continue }
        };
        let last = fixings.iter().map(|(d, _)| *d).max().unwrap_or(first);
        let mut to = last;
        if last < end {
            match get(source, daily_path(), vec![("date", end.format("%d.%m.%Y").to_string())]).await {
                Ok(body) if daily_date(&body) == Some(last) => to = end,
                Ok(_) => {}
                Err(e) => crate::engine::log(&format!("[TaxReport] Failed to check the ČNB fixing of {end}: {e}")),
            }
        }
        let stored = db.run(move |c, _| {
            let tx = c.unchecked_transaction().map_err(|e| e.to_string())?;
            for (day, rates) in &fixings {
                for (currency, rate) in rates {
                    tx.execute("INSERT INTO cnb_rates (currency, date, rate) VALUES (?1, ?2, ?3) ON CONFLICT (currency, date) DO UPDATE SET rate = excluded.rate",
                               rusqlite::params![currency, day.to_string(), rate.to_s_f()]).map_err(|e| e.to_string())?;
                }
            }
            tx.execute("INSERT INTO cnb_coverage (from_date, to_date) VALUES (?1, ?2)", rusqlite::params![first.to_string(), to.to_string()])
                .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())
        }).await;
        if let Err(e) = stored { crate::engine::log(&format!("[TaxReport] Failed to store ČNB rates of {year}: {e}")); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venue::http::ScriptedTransport;

    fn day(s: &str) -> NaiveDate { date(s).unwrap() }

    fn db() -> Db {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../../migrations/20261105000001_cnb_rates.sql")).unwrap();
        Db::new(c, crate::crypto::Cipher::new(&crate::crypto::EncryptionKeys::resolve(&|_| None, "cnb").unwrap()))
    }

    async fn table(db: &Db) -> Cnb { db.run(|c, _| Cnb::load(c).map_err(|e| format!("{e:?}"))).await.unwrap() }

    fn year_key(year: i32) -> String { format!("GET {}?year={year}", year_path()) }
    fn daily_key(d: &str) -> String { format!("GET {}?date={d}", daily_path()) }
    fn text(t: &ScriptedTransport, key: &str, body: &str) { t.reply_typed(key, 200, "text/plain", body); }

    fn rate(cnb: &Cnb, currency: &str, d: &str) -> Option<String> { cnb.rate(currency, day(d)).map(Dec::to_s_f) }

    /// The header repeated mid-file with another currency set, a `100 JPY` column read per unit, a currency absent from
    /// the latest fixing missing (never an older one), 1 January read from the previous year's last fixing, and a complete
    /// year (the daily answer for 31 December dated 31 December) covered to its end.
    #[tokio::test(flavor = "current_thread")]
    async fn year_files_by_their_headers_and_the_new_year_from_the_old() {
        let db = db();
        let t = ScriptedTransport::default();
        text(&t, &year_key(2025), "Date|1 EUR|1 USD|100 JPY\n30.12.2025|24.300|20.800|13.500\n31.12.2025|24.275|20.750|13.400\n");
        text(&t, &year_key(2026), "Date|1 EUR|1 USD|100 JPY\n02.01.2026|24.250|20.700|13.300\nDate|1 EUR|100 JPY\n05.01.2026|24.200|13.200\n");
        text(&t, &daily_key("09.01.2026"), "09 Jan 2026 #6\nCountry|Currency|Amount|Code|Rate\n");
        let source = Source::Scripted(t.clone());
        ensure_loaded(&db, Some(&source), vec![2025, 2026], day("2026-01-10")).await;
        let cnb = table(&db).await;
        assert_eq!(rate(&cnb, "JPY", "2025-12-30").as_deref(), Some("0.135"));
        assert_eq!(rate(&cnb, "EUR", "2026-01-01").as_deref(), Some("24.275"), "1 January: 31 December's fixing");
        assert_eq!(rate(&cnb, "USD", "2026-01-03").as_deref(), Some("20.7"), "a Saturday: Friday's");
        assert_eq!(rate(&cnb, "USD", "2026-01-05"), None, "absent from the latest fixing: missing, not Friday's");
        assert_eq!(rate(&cnb, "EUR", "2026-01-05").as_deref(), Some("24.2"));
        assert_eq!(rate(&cnb, "EUR", "2026-01-09"), None, "past the coverage: missing, not 5 January's");
        // 2026's file ends 5 January but the daily answer for 9 January is dated 9 January: covered to 5 January only.
        assert!(!cnb.covered(day("2026-01-06")) && cnb.covered(day("2026-01-05")));
        assert!(cnb.covered(day("2025-01-01")) && cnb.covered(day("2025-12-31")), "2025 ends on its last fixing, 31 December");
    }

    /// A body cut mid-row, or a row before any header, is refused whole: nothing stored, nothing covered; the next run
    /// fetches the year again.
    #[tokio::test(flavor = "current_thread")]
    async fn a_truncated_body_is_refused_and_fetched_again() {
        let db = db();
        let t = ScriptedTransport::default();
        text(&t, &year_key(2025), "Date|1 EUR|1 USD\n02.01.2025|25.100|24.\n03.01.2025|25.1");
        text(&t, &year_key(2025), "Date|1 EUR|1 USD\n02.01.2025|25.100|24.000\n31.12.2025|25.000|22.700\n");
        let source = Source::Scripted(t.clone());
        ensure_loaded(&db, Some(&source), vec![2025], day("2026-03-10")).await;
        let cnb = table(&db).await;
        assert!(cnb.rate("EUR", day("2025-01-02")).is_none() && cnb.coverage.is_empty());
        ensure_loaded(&db, Some(&source), vec![2025], day("2026-03-10")).await;
        assert_eq!(rate(&table(&db).await, "USD", "2025-06-01").as_deref(), Some("24.0"));
        assert!(parse_year("02.01.2025|25.1\n", 2025).is_err() && parse_year("Date|1 EUR\n02.01.2025|25.1|1\n", 2025).is_err());
        assert!(parse_year("Date|1 EUR\n02.01.2024|25.1\n", 2025).is_err(), "a row of another year");
        // Cut inside the last rate or before the last line break: syntactically valid lines, still refused.
        assert!(parse_year("Date|1 EUR|1 USD\n31.12.2025|25.000|2\n", 2025).is_err());
        assert!(parse_year("Date|1 EUR|1 USD\n31.12.2025|25.000|21.368", 2025).is_err());
        assert!(parse_year("Date|1 EUR|1 USD\n31.12.2025|25.000|21.368\n", 2025).is_ok());
    }

    /// A past year whose file ends early covers only to its last row when the daily answer for 31 December is dated later
    /// (a truncated file); a file ending 15 January likewise.
    #[tokio::test(flavor = "current_thread")]
    async fn a_past_year_ending_early_covers_to_its_last_fixing() {
        let db = db();
        let t = ScriptedTransport::default();
        text(&t, &year_key(2025), "Date|1 EUR\n15.01.2025|25.000\n");
        text(&t, &daily_key("31.12.2025"), "31 Dec 2025 #250\n");
        text(&t, &year_key(2024), "Date|1 EUR\n27.12.2024|25.200\n29.12.2024|25.150\n");
        text(&t, &daily_key("31.12.2024"), "31 Dec 2024 #251\n");
        ensure_loaded(&db, Some(&Source::Scripted(t.clone())), vec![2024, 2025], day("2026-03-10")).await;
        let cnb = table(&db).await;
        assert!(cnb.covered(day("2025-01-15")) && !cnb.covered(day("2025-01-16")));
        assert!(cnb.covered(day("2024-12-29")) && !cnb.covered(day("2024-12-30")), "29 December, the daily answer dated 31 December");
    }

    /// The current year: a fetch on Tuesday 10 March 2026 whose file ends Friday 6 March. The daily answer for Monday 9 March
    /// dated 9 March: covered to 6 March only; dated 6 March (Monday a holiday): covered to 9 March.
    #[tokio::test(flavor = "current_thread")]
    async fn the_current_year_to_yesterday_only_when_the_daily_fixing_proves_it() {
        for (answer, to) in [("09 Mar 2026 #48", "2026-03-06"), ("06 Mar 2026 #47", "2026-03-09")] {
            let db = db();
            let t = ScriptedTransport::default();
            text(&t, &year_key(2026), "Date|1 EUR|1 USD\n05.03.2026|25.000|21.400\n06.03.2026|25.000|21.368\n");
            text(&t, &daily_key("09.03.2026"), &format!("{answer}\n"));
            ensure_loaded(&db, Some(&Source::Scripted(t.clone())), vec![2026], day("2026-03-10")).await;
            let cnb = table(&db).await;
            assert!(cnb.covered(day(to)) && !cnb.covered(day(to).succ_opt().unwrap()), "{answer}");
            let saturday = (to == "2026-03-09").then_some("21.368");
            assert_eq!(cnb.rate("USD", day("2026-03-07")).map(Dec::to_s_f).as_deref(), saturday, "{answer}: Saturday reads Friday only when covered");
        }
    }
}
