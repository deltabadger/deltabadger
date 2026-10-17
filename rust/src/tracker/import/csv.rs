//! What Import::Run and Ruby's CSV give the import readers: the upload as text (`as_text`), the rows
//! (`CSV.parse(text, headers: true)`, csv 3.3.6), the filename's zone (`offset_from`), a cell's time (`time`) and a cell's
//! amount (`amount`). Pinned by rust/tests/fixtures/import_vectors.json (script/rust/record_import_vectors.rb).
use crate::ruby::{self, BigDec};
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use regex::Regex;
use std::sync::LazyLock;

/// CSV::MalformedCSVError or ArgumentError: either way the file is refused as `:malformed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed(pub String);

/// Import::Run.as_text: invalid UTF-8 scrubbed to U+FFFD (Ruby's `scrub` replaces each maximal invalid subpart, as
/// from_utf8_lossy does), then one leading byte order mark dropped.
pub fn as_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.strip_prefix('\u{FEFF}').unwrap_or(&text).to_string()
}

/// One CSV row: a cell is None when it was empty and unquoted, as Ruby's CSV leaves it nil.
pub type Row = Vec<Option<String>>;

/// `CSV.parse(text, headers: true)`: the first row is the headers, every later row a record.
#[derive(Debug, Clone, PartialEq)]
pub struct Table { pub headers: Row, pub rows: Vec<Row> }

impl Table {
    pub fn parse(text: &str) -> Result<Self, Malformed> {
        let mut reader = Reader::new(text);
        let headers = reader.next().transpose()?.unwrap_or_default();
        Ok(Self { headers, rows: reader.collect::<Result<_, _>>()? })
    }
    pub fn has_header(&self, name: &str) -> bool { self.headers.iter().any(|h| h.as_deref() == Some(name)) }
    /// `row[name]`: the cell under the FIRST header of that name; nil past a short row's end.
    pub fn get<'a>(&self, row: &'a Row, name: &str) -> Option<&'a str> {
        let i = self.headers.iter().position(|h| h.as_deref() == Some(name))?;
        row.get(i)?.as_deref()
    }
}

/// `CSV.parse_line(text)`: the first row alone; what follows it is never read.
pub fn first_row(text: &str) -> Result<Option<Row>, Malformed> { Reader::new(text).next().transpose() }

/// csv's parser over a String: the row separator is detected from the whole text (the first "\r\n", "\r" or "\n",
/// quotes not considered; "\n" when there is none). A quoted cell holds anything, `""` being one quote, and must be
/// followed by a comma, the separator or the end. An unquoted cell may hold no quote, and under "\r\n" no CR or LF.
/// An empty line is a row with no cells.
struct Reader<'a> { s: &'a [u8], pos: usize, sep: &'static [u8] }

impl<'a> Reader<'a> {
    fn new(text: &'a str) -> Self {
        let s = text.as_bytes();
        let lf = s.iter().position(|&b| b == b'\n');
        let cr = s[..lf.unwrap_or(s.len())].iter().position(|&b| b == b'\r');
        let sep: &'static [u8] = match (cr, lf) {
            (Some(c), Some(l)) if c + 1 == l => b"\r\n",
            (Some(_), _) => b"\r",
            _ => b"\n",
        };
        Self { s, pos: 0, sep }
    }
    fn at_sep(&self, i: usize) -> bool { self.s[i..].starts_with(self.sep) }
    fn malformed(&self, what: &str) -> Malformed { Malformed(format!("{what} at byte {}", self.pos)) }
    fn text(bytes: &[u8]) -> String { String::from_utf8_lossy(bytes).into_owned() } // whole UTF-8: cut at ASCII only

    fn row(&mut self) -> Result<Row, Malformed> {
        if self.at_sep(self.pos) { self.pos += self.sep.len(); return Ok(vec![]); }
        let mut row = vec![];
        loop {
            if self.s.get(self.pos) == Some(&b'"') {
                self.pos += 1;
                let mut cell = vec![];
                loop {
                    let Some(q) = self.s[self.pos..].iter().position(|&b| b == b'"').map(|q| q + self.pos) else {
                        return Err(self.malformed("unclosed quoted field"));
                    };
                    cell.extend_from_slice(&self.s[self.pos..q]);
                    self.pos = q + 1;
                    if self.s.get(self.pos) == Some(&b'"') { cell.push(b'"'); self.pos += 1; } else { break; }
                }
                row.push(Some(Self::text(&cell)));
                if self.pos < self.s.len() && self.s[self.pos] != b',' && !self.at_sep(self.pos) {
                    return Err(self.malformed("a value after a quoted field"));
                }
            } else {
                let start = self.pos;
                while self.pos < self.s.len() && self.s[self.pos] != b',' && !self.at_sep(self.pos) { self.pos += 1; }
                let cell = &self.s[start..self.pos];
                if cell.contains(&b'"') { return Err(self.malformed("illegal quoting")); }
                if self.sep == b"\r\n" && cell.iter().any(|&b| b == b'\r' || b == b'\n') {
                    return Err(self.malformed("a new line in an unquoted field"));
                }
                row.push((!cell.is_empty()).then(|| Self::text(cell)));
            }
            if self.pos >= self.s.len() { return Ok(row); }
            if self.s[self.pos] == b',' { self.pos += 1; continue; }
            self.pos += self.sep.len(); // at the separator
            return Ok(row);
        }
    }
}

impl Iterator for Reader<'_> {
    type Item = Result<Row, Malformed>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.s.len() { return None; }
        let row = self.row();
        if row.is_err() { self.pos = self.s.len(); }
        Some(row)
    }
}

/// Import::BinanceCsv::FILENAME_OFFSET (Ruby's \d is ASCII).
static FILENAME_OFFSET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(UTC([+-])([0-9]{1,2})(?::([0-5][0-9]))?\)").unwrap());
/// Import::BinanceCsv::UTC_TIME.
pub const UTC_TIME: &str = "UTC_Time";

/// Import::BinanceCsv.offset_from: "+00:00" for a file whose header names UTC_Time, else the filename's "(UTC±H[:MM])"
/// as "±HH:MM" (a zero offset is "+"), else None.
pub fn offset_from(filename: Option<&str>, bytes: Option<&[u8]>) -> Option<String> {
    let utc = bytes.is_some_and(|b| first_row(&as_text(b)).ok().flatten().unwrap_or_default().iter().any(|h| h.as_deref() == Some(UTC_TIME)));
    if utc { return Some("+00:00".into()); }
    let m = FILENAME_OFFSET.captures(filename.unwrap_or(""))?;
    let hours: u32 = m[2].parse().ok()?;
    let minutes: u32 = m.get(3).map_or(Some(0), |x| x.as_str().parse().ok())?;
    let sign = if hours == 0 && minutes == 0 { "+" } else { &m[1] };
    Some(format!("{sign}{hours:02}:{minutes:02}"))
}

/// The time spellings Import::Run.time is known to accept (Date._parse's grammar is far wider; anything outside these
/// is refused, which only ever refuses a file Ruby would have read): a date as YYYY-MM-DD, YY-MM-DD, YYYY/MM/DD or
/// "D Mon YYYY", a space or T, H:MM:SS with an optional fraction, then an optional zone of its own (Z or ±HH:MM).
static TIME: LazyLock<Regex> = LazyLock::new(|| Regex::new(concat!(
    r"^(?:(?P<y>[0-9]{4}|[0-9]{2})-(?P<m>[0-9]{1,2})-(?P<d>[0-9]{1,2})|(?P<sy>[0-9]{4})/(?P<sm>[0-9]{1,2})/(?P<sd>[0-9]{1,2})",
    r"|(?P<nd>[0-9]{1,2}) +(?P<nm>(?i:jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec))[a-zA-Z]* +(?P<ny>[0-9]{4}))",
    r"(?: +|T)(?P<H>[0-9]{1,2}):(?P<M>[0-9]{2}):(?P<S>[0-9]{2})(?:\.(?P<f>[0-9]+))?",
    r"(?P<z>Z|[+-][0-9]{2}:[0-9]{2})?$")).unwrap());
const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

/// Import::Run.time: a real date and time of day, read in its own zone if it states one, else at `offset` ("±HH:MM",
/// as Time.parse reads the text with the offset appended), else in UTC (the process zone the app runs in).
pub fn time(value: Option<&str>, offset: Option<&str>) -> Result<DateTime<Utc>, Malformed> {
    let text = ruby::strip(value.unwrap_or(""));
    let bad = || Malformed(format!("unreadable time {text:?}"));
    let c = TIME.captures(text).ok_or_else(bad)?;
    let num = |name: &str| c.name(name).map(|m| m.as_str().parse::<u32>().expect("digits"));
    let (year, month, day) = if let Some(y) = c.name("y") {
        let mut year = y.as_str().parse::<i32>().map_err(|_| bad())?;
        if y.as_str().len() == 2 { year += if year >= 69 { 1900 } else { 2000 }; } // Date._parse's two-digit year
        (year, num("m"), num("d"))
    } else if c.name("sy").is_some() {
        (num("sy").unwrap_or(0) as i32, num("sm"), num("sd"))
    } else {
        let month = c.name("nm").map(|m| m.as_str().to_ascii_lowercase()).and_then(|m| MONTHS.iter().position(|x| *x == m));
        (num("ny").unwrap_or(0) as i32, month.map(|m| m as u32 + 1), num("nd"))
    };
    let date = NaiveDate::from_ymd_opt(year, month.ok_or_else(bad)?, day.ok_or_else(bad)?).ok_or_else(bad)?;
    let nanos = c.name("f").map_or(0, |f| format!("{:0<9}", &f.as_str()[..f.as_str().len().min(9)]).parse::<u32>().expect("digits"));
    let (h, m, s) = (num("H").unwrap_or(0), num("M").unwrap_or(0), num("S").unwrap_or(0));
    if h >= 24 || m >= 60 || s >= 60 { return Err(bad()); }
    let wall = NaiveDateTime::new(date, NaiveTime::from_hms_nano_opt(h, m, s, nanos).ok_or_else(bad)?);
    let zone = match c.name("z").map(|z| z.as_str()).or(offset) {
        None | Some("Z") => FixedOffset::east_opt(0),
        Some(z) => {
            let (hh, mm) = (z[1..3].parse::<i32>().map_err(|_| bad())?, z[4..6].parse::<i32>().map_err(|_| bad())?);
            let secs = hh * 3600 + mm * 60;
            // Time.parse refuses an offset of a day or more ("utc_offset out of range").
            FixedOffset::east_opt(if z.starts_with('-') { -secs } else { secs })
        }
    }.ok_or_else(bad)?;
    Ok(zone.from_local_datetime(&wall).single().ok_or_else(bad)?.with_timezone(&Utc))
}

/// Import::Run::DECIMAL (Ruby's \d is ASCII).
static DECIMAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([+-]?)(?:([0-9]+)(?:\.([0-9]*))?|\.([0-9]+))(?:[eE]([+-]?[0-9]+))?$").unwrap());

/// Import::Run.amount: a plain decimal (an exponent allowed), read exactly as `to_d` reads it. BigDec's own bounds
/// (MAX_INPUT_LEN characters, 10^±MAX_EXPONENT, MAX_DIGITS digits) also refuse a value Ruby would have read.
pub fn amount(value: Option<&str>) -> Result<BigDec, Malformed> {
    let text = ruby::strip(value.unwrap_or(""));
    let bad = || Malformed(format!("unreadable amount {text:?}"));
    let c = DECIMAL.captures(text).ok_or_else(bad)?;
    let part = |i: usize| c.get(i).map_or("", |m| m.as_str());
    let int = if part(2).is_empty() { "0" } else { part(2) };
    let frac = if part(3).is_empty() { part(4) } else { part(3) };
    let exp = if part(5).is_empty() { "0" } else { part(5) };
    BigDec::parse(&format!("{}{int}.{}e{exp}", part(1), if frac.is_empty() { "0" } else { frac })).map_err(|_| bad())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(s: &str) -> Result<(Row, Vec<Row>), Malformed> { Table::parse(s).map(|t| (t.headers, t.rows)) }
    fn some(v: &[&str]) -> Row { v.iter().map(|x| Some(x.to_string())).collect() }

    #[test]
    fn rows_as_ruby_csv_reads_them() {
        assert_eq!(table("a,b\n1,2\r\n3,4\n").unwrap().1, vec![some(&["1", "2\r"]), some(&["3", "4"])]);
        assert!(table("a,b\r\n1,x\ry\r\n").is_err());
        assert_eq!(table("a,b\r1,x\ny\r").unwrap().1, vec![some(&["1", "x\ny"])]);
        assert!(table("a,b\n1,\"x\"\r\n").is_err());
        assert_eq!(table("\n").unwrap(), (vec![], vec![]));
        assert_eq!(table(",\n").unwrap(), (vec![None, None], vec![]));
        assert_eq!(table("a\n\"\"\n").unwrap().1, vec![some(&[""])]);
        assert!(table("a\n\"\n").is_err());
        assert_eq!(table("a,b\n1,2\n\n\n").unwrap().1, vec![some(&["1", "2"]), vec![], vec![]]);
        assert_eq!(table("a\n\"x\"\"\"\n").unwrap().1, vec![some(&["x\""])]);
        assert!(table("a\nx\"\n").is_err());
        assert!(table("a,b\n \"x\",1\n").is_err());
        assert_eq!(first_row("a,UTC_Time\n\"unclosed").unwrap(), Some(some(&["a", "UTC_Time"])));
        assert_eq!(first_row("").unwrap(), None);
    }

    #[test]
    fn a_short_row_is_nil_and_the_first_duplicate_header_wins() {
        let t = Table::parse("a,b,a\n1\n2,3,4\n").unwrap();
        assert_eq!((t.get(&t.rows[0], "b"), t.get(&t.rows[1], "a")), (None, Some("2")));
    }
}
