//! What Import::Run and 2.x's CSV give the import readers: the upload as text (`as_text`), the rows
//! (`CSV.parse(text, headers: true)`, csv 3.3.6), the filename's zone (`offset_from`), a cell's time (`time`) and a cell's
//! amount (`amount`). Pinned by rust/tests/fixtures/import_vectors.json.
use crate::v2::{self, BigDec};
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use regex::Regex;
use std::borrow::Cow;
use std::sync::LazyLock;

/// CSV::MalformedCSVError or ArgumentError: either way the file is refused as `:malformed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed(pub String);

/// Import::Run.as_text: invalid UTF-8 scrubbed to U+FFFD (2.x's `scrub` replaces each maximal invalid subpart, as
/// from_utf8_lossy does), then one leading byte order mark dropped.
pub fn as_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.strip_prefix('\u{FEFF}').unwrap_or(&text).to_string()
}

/// One record's kept cells, in the order of the table's kept columns: a cell is None when it was empty and unquoted (as
/// 2.x's CSV leaves it nil) or past a short record's end. `blank`: a line with no cells at all (it keeps nothing).
#[derive(Debug, Clone, PartialEq)]
pub struct Record<'a> { pub blank: bool, cells: Vec<Option<Cow<'a, str>>> }

/// `CSV.parse(text, headers: true)`, streamed (PLAN-710-importmem B2′): the first row is the headers, every later row a
/// record. Only the columns a reader names are kept (the first of each name, as `row[name]` reads it); every other cell
/// is tokenized, so its quoting is still checked, and dropped without being kept. `parse` checks the whole text before it
/// answers, so a malformed record anywhere refuses the file before any header or row is judged; `records` then reads the
/// rows one at a time, each kept cell borrowed from the text (owned only where a doubled quote is undone).
#[derive(Debug, Clone)]
pub struct Table<'a> { text: &'a str, sep: &'static [u8], body: usize, width: usize, columns: Vec<(String, usize)> }

impl<'a> Table<'a> {
    /// The whole text checked, the header row read for the columns `keep` names.
    pub fn parse(text: &'a str, keep: impl Fn(&str) -> bool) -> Result<Self, Malformed> {
        let table = Self::header(text, keep)?;
        let mut reader = Reader { s: text.as_bytes(), text, pos: table.body, sep: table.sep };
        while let Some(record) = reader.next(|_, _| {}) { record?; }
        Ok(table)
    }
    /// `CSV.parse_line(text)`'s header row alone, for the columns `keep` names: what follows it is never read.
    pub fn header(text: &'a str, keep: impl Fn(&str) -> bool) -> Result<Self, Malformed> {
        let mut reader = Reader::new(text);
        let (mut width, mut columns) = (0, Vec::<(String, usize)>::new());
        if let Some(header) = reader.next(|i, cell| {
            width = i + 1;
            if let Some(name) = cell.value() { if keep(&name) && !columns.iter().any(|(c, _)| *c == name) { columns.push((name.into_owned(), i)); } }
        }) { header?; }
        Ok(Self { text, sep: reader.sep, body: reader.pos, width, columns })
    }
    pub fn has_header(&self, name: &str) -> bool { self.columns.iter().any(|(c, _)| c == name) }
    /// The header row's cell count.
    pub fn width(&self) -> usize { self.width }
    /// The position of the first header of that name, among the kept ones.
    pub fn position(&self, name: &str) -> Option<usize> { self.columns.iter().find(|(c, _)| c == name).map(|(_, i)| *i) }
    /// The kept headers, in the header row's order.
    pub fn headers(&self) -> impl Iterator<Item = &str> { self.columns.iter().map(|(c, _)| c.as_str()) }
    /// The records, read one at a time.
    pub fn records(&self) -> impl Iterator<Item = Record<'a>> + '_ {
        let mut reader = Reader { s: self.text.as_bytes(), text: self.text, pos: self.body, sep: self.sep };
        std::iter::from_fn(move || {
            let mut cells: Vec<Option<Cow<'a, str>>> = vec![];
            let columns = &self.columns;
            let read = reader.next(|i, cell| {
                if let Some(slot) = columns.iter().position(|(_, at)| *at == i) {
                    if cells.is_empty() { cells.resize(columns.len(), None); }
                    cells[slot] = cell.value();
                }
            })?;
            // `parse` checked every record: none fails here.
            let blank = !read.ok()?;
            Some(Record { blank, cells })
        })
    }
    /// `row[name]`: the cell under the FIRST header of that name; nil past a short row's end.
    pub fn get<'r>(&self, record: &'r Record<'a>, name: &str) -> Option<&'r str> {
        let slot = self.columns.iter().position(|(c, _)| c == name)?;
        record.cells.get(slot)?.as_deref()
    }
}

/// One cell as the tokenizer found it: an unquoted cell's text, or a quoted cell's text with its `""` still doubled.
#[derive(Clone, Copy)]
enum Cell<'a> { Plain(&'a str), Quoted(&'a str) }

impl<'a> Cell<'a> {
    fn value(self) -> Option<Cow<'a, str>> {
        match self {
            Cell::Plain("") => None,
            Cell::Plain(s) => Some(Cow::Borrowed(s)),
            Cell::Quoted(s) if s.contains("\"\"") => Some(Cow::Owned(s.replace("\"\"", "\""))),
            Cell::Quoted(s) => Some(Cow::Borrowed(s)),
        }
    }
}

/// csv's parser over a String: the row separator is detected from the whole text (the first "\r\n", "\r" or "\n",
/// quotes not considered; "\n" when there is none). A quoted cell holds anything, `""` being one quote, and must be
/// followed by a comma, the separator or the end. An unquoted cell may hold no quote, and under "\r\n" no CR or LF.
/// An empty line is a row with no cells. Each cell is handed to the caller as found (cut at ASCII bytes only, so every
/// cell is whole UTF-8); none is kept here.
struct Reader<'a> { text: &'a str, s: &'a [u8], pos: usize, sep: &'static [u8] }

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
        Self { text, s, pos: 0, sep }
    }
    fn at_sep(&self, i: usize) -> bool { self.s[i..].starts_with(self.sep) }
    fn malformed(&self, what: &str) -> Malformed { Malformed(format!("{what} at byte {}", self.pos)) }

    /// One row, each cell handed to `each` with its position. Ok(false): an empty line, no cells.
    fn row(&mut self, each: &mut impl FnMut(usize, Cell<'a>)) -> Result<bool, Malformed> {
        if self.at_sep(self.pos) { self.pos += self.sep.len(); return Ok(false); }
        let mut i = 0;
        loop {
            if self.s.get(self.pos) == Some(&b'"') {
                self.pos += 1;
                let start = self.pos;
                loop {
                    let Some(q) = self.s[self.pos..].iter().position(|&b| b == b'"').map(|q| q + self.pos) else {
                        return Err(self.malformed("unclosed quoted field"));
                    };
                    self.pos = q + 1;
                    if self.s.get(self.pos) == Some(&b'"') { self.pos += 1; } else { break; }
                }
                each(i, Cell::Quoted(&self.text[start..self.pos - 1]));
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
                each(i, Cell::Plain(&self.text[start..self.pos]));
            }
            i += 1;
            if self.pos >= self.s.len() { return Ok(true); }
            if self.s[self.pos] == b',' { self.pos += 1; continue; }
            self.pos += self.sep.len(); // at the separator
            return Ok(true);
        }
    }

    /// The next row (None past the end); after a malformed one, nothing more.
    fn next(&mut self, mut each: impl FnMut(usize, Cell<'a>)) -> Option<Result<bool, Malformed>> {
        if self.pos >= self.s.len() { return None; }
        let row = self.row(&mut each);
        if row.is_err() { self.pos = self.s.len(); }
        Some(row)
    }
}

/// Import::BinanceCsv::FILENAME_OFFSET (2.x's \d is ASCII).
static FILENAME_OFFSET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(UTC([+-])([0-9]{1,2})(?::([0-5][0-9]))?\)").unwrap());
/// Import::BinanceCsv::UTC_TIME.
pub const UTC_TIME: &str = "UTC_Time";

/// Import::BinanceCsv.offset_from: "+00:00" for a file whose header names UTC_Time, else the filename's "(UTC±H[:MM])"
/// as "±HH:MM" (a zero offset is "+"), else None.
pub fn offset_from(filename: Option<&str>, bytes: Option<&[u8]>) -> Option<String> {
    // B2′: the header row alone, and only whether it names UTC_Time.
    let utc = bytes.is_some_and(|b| Table::header(&as_text(b), |h| h == UTC_TIME).is_ok_and(|t| t.has_header(UTC_TIME)));
    if utc { return Some("+00:00".into()); }
    let m = FILENAME_OFFSET.captures(filename.unwrap_or(""))?;
    let hours: u32 = m[2].parse().ok()?;
    let minutes: u32 = m.get(3).map_or(Some(0), |x| x.as_str().parse().ok())?;
    let sign = if hours == 0 && minutes == 0 { "+" } else { &m[1] };
    Some(format!("{sign}{hours:02}:{minutes:02}"))
}

/// The time spellings Import::Run.time is known to accept (Date._parse's grammar is far wider; anything outside these
/// is refused, which only ever refuses a file 2.x would have read): a date as YYYY-MM-DD, YY-MM-DD, YYYY/MM/DD or
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
    let text = v2::strip(value.unwrap_or(""));
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

/// Import::Run::DECIMAL (2.x's \d is ASCII).
static DECIMAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([+-]?)(?:([0-9]+)(?:\.([0-9]*))?|\.([0-9]+))(?:[eE]([+-]?[0-9]+))?$").unwrap());

/// Import::Run.amount: a plain decimal (an exponent allowed), read exactly as `to_d` reads it. BigDec's own bounds
/// (MAX_INPUT_LEN characters, 10^±MAX_EXPONENT, MAX_DIGITS digits) also refuse a value 2.x would have read.
pub fn amount(value: Option<&str>) -> Result<BigDec, Malformed> {
    let text = v2::strip(value.unwrap_or(""));
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

    type Row = Vec<Option<String>>;
    /// Every column kept, as the whole-table parse returned it: the headers, then each record's cells (none past its end).
    fn table(s: &str) -> Result<(Row, Vec<Row>), Malformed> {
        let t = Table::parse(s, |_| true)?;
        let mut headers: Row = vec![None; t.width()];
        for (name, i) in &t.columns { headers[*i] = Some(name.clone()); }
        let mut rows = vec![];
        let mut reader = Reader { s: s.as_bytes(), text: s, pos: t.body, sep: t.sep };
        loop {
            let mut row: Row = vec![];
            let Some(read) = reader.next(|i, cell| { row.resize(i + 1, None); row[i] = cell.value().map(Cow::into_owned); }) else { break };
            read?;
            rows.push(row);
        }
        Ok((headers, rows))
    }
    fn some(v: &[&str]) -> Row { v.iter().map(|x| Some(x.to_string())).collect() }
    fn first_row(s: &str) -> Result<Option<Row>, Malformed> {
        let t = Table::header(s, |_| true)?;
        Ok((!s.is_empty()).then(|| { let mut h: Row = vec![None; t.width()]; for (n, i) in &t.columns { h[*i] = Some(n.clone()); } h }))
    }

    #[test]
    fn rows_as_v2_csv_reads_them() {
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
        let t = Table::parse("a,b,a\n1\n2,3,4\n", |_| true).unwrap();
        let rows: Vec<Record> = t.records().collect();
        assert_eq!((t.get(&rows[0], "b"), t.get(&rows[1], "a")), (None, Some("2")));
    }

    /// B2′: a column no reader names is never kept, but its quoting is still checked; a blank line keeps nothing.
    #[test]
    fn only_the_named_columns_are_kept_and_every_cell_is_checked() {
        let t = Table::parse("x,a,y\n\"p\"\"q\",1,\"r\"\n\n,2\n", |h| h == "a").unwrap();
        assert_eq!((t.width(), t.headers().collect::<Vec<_>>(), t.position("a"), t.has_header("x")), (3, vec!["a"], Some(1), false));
        let rows: Vec<Record> = t.records().collect();
        assert_eq!(rows.iter().map(|r| (r.blank, t.get(r, "a"))).collect::<Vec<_>>(), vec![(false, Some("1")), (true, None), (false, Some("2"))]);
        assert_eq!(rows[1].cells.capacity(), 0, "a blank line allocates nothing");
        assert!(Table::parse("x,a\n1,2\n\"broken,3\n", |h| h == "a").is_err(), "a broken quote in a dropped column");
        assert!(Table::parse("x,a\n1,2\nx\"y,3\n", |h| h == "a").is_err());
    }
}
