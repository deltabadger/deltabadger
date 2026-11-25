//! `multipart/form-data` (RFC 7578), read by `entry` for the one route that takes a file:
//! POST /tracker/import. Text parts become form fields; the part named `file` is kept as raw bytes
//! with its filename, as 2.x's `params[:file].read.b` and `original_filename`.

/// The most parts one body may hold. The import form posts four.
pub const PARTS: usize = 64;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub fields: Vec<(String, String)>,
    /// The `file` part's bytes and filename. A part whose filename is empty (a file input left
    /// empty) is no file, as Rack drops it.
    pub file: Option<(Vec<u8>, String)>,
    /// Every other file-valued part, by name, with its contents read as text (PLAN-710-confirm rule N2: a request is
    /// classified from every part it carries, before any is dropped).
    pub file_parts: Vec<(String, String)>,
    /// How many file-valued parts are named `file` (an empty one included).
    pub files: usize,
    /// PLAN-710-mcpadapter I3/J2/J7: some part reads two ways (two dispositions, two `name`s, two `filename`s or two
    /// `filename*`s). Reported, not decided: the part is read as before, and the import callers refuse the request.
    pub ambiguous: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error { Malformed, TooManyParts }

/// The `boundary` parameter of a `multipart/form-data` content type.
pub fn boundary(content_type: &str) -> Option<String> {
    let (media, rest) = content_type.split_once(';')?;
    if !media.trim().eq_ignore_ascii_case("multipart/form-data") { return None; }
    params(rest).find(|(k, _)| k.eq_ignore_ascii_case("boundary")).map(|(_, v)| v.text())
        .filter(|b| !b.is_empty() && b.len() <= 70)
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|i| i + from)
}

/// D-MULTIPART-DELIMITER: what follows `--boundary` at `i`, when it makes a delimiter line: `--` (the closing one) or
/// nothing, optional blanks, then CRLF, or the body's end after the closing one. `(closing, where the next part starts)`.
fn tail(body: &[u8], mut i: usize) -> Option<(bool, usize)> {
    let closing = body.get(i..).is_some_and(|rest| rest.starts_with(b"--"));
    if closing { i += 2; }
    while matches!(body.get(i), Some(b' ' | b'\t')) { i += 1; }
    if body.get(i..).is_some_and(|rest| rest.starts_with(b"\r\n")) { return Some((closing, i + 2)); }
    (closing && i == body.len()).then_some((true, i))
}

/// The first delimiter line at or after `from` (`between` is CRLF `--boundary`): where its CRLF starts, and its `tail`.
/// A line that only begins like one is content.
fn delimiter(body: &[u8], between: &[u8], mut from: usize) -> Option<(usize, bool, usize)> {
    loop {
        let at = find(body, between, from)?;
        if let Some((closing, next)) = tail(body, at + between.len()) { return Some((at, closing, next)); }
        from = at + 1;
    }
}

pub fn parse(body: &[u8], boundary: &str) -> Result<Parsed, Error> {
    let delimiter_text = format!("--{boundary}");
    let between = format!("\r\n--{boundary}");
    // Anything before the first delimiter is a preamble and ignored.
    let first = if body.starts_with(delimiter_text.as_bytes()) { tail(body, delimiter_text.len()) } else { None };
    let (mut closing, mut at) = match first {
        Some(found) => found,
        None => delimiter(body, between.as_bytes(), 0).map(|(_, closing, next)| (closing, next)).ok_or(Error::Malformed)?,
    };
    let mut parsed = Parsed::default();
    let mut parts = 0;
    loop {
        if closing { return Ok(parsed); }
        parts += 1;
        if parts > PARTS { return Err(Error::TooManyParts); }
        if body[at..].starts_with(b"\r\n") { return Err(Error::Malformed); } // a part with no headers names nothing
        let head_end = find(body, b"\r\n\r\n", at).ok_or(Error::Malformed)?;
        let content_start = head_end + 4;
        let (end, close, next) = delimiter(body, between.as_bytes(), content_start).ok_or(Error::Malformed)?;
        let head = std::str::from_utf8(&body[at..head_end]).map_err(|_| Error::Malformed)?;
        let mut dispositions = head.split("\r\n")
            .filter_map(|line| line.split_once(':'))
            .filter(|(name, _)| name.trim().eq_ignore_ascii_case("content-disposition"))
            .map(|(_, value)| value);
        let disposition = dispositions.next().ok_or(Error::Malformed)?;
        if dispositions.next().is_some() { parsed.ambiguous = true; }
        let (kind, rest) = disposition.split_once(';').unwrap_or((disposition, ""));
        if !kind.trim().eq_ignore_ascii_case("form-data") { return Err(Error::Malformed); }
        // PLAN-710-importmem C1: one pass over the parameters, the last `name`, `filename` and `filename*` each kept (as
        // ever), every other one read past without being kept. PLAN-710-mcpadapter J7: one given twice is reported.
        let (mut name, mut plain, mut extended) = (None, None, None);
        for (key, value) in params(rest) {
            let slot = if key.eq_ignore_ascii_case("name") { &mut name }
                else if key.eq_ignore_ascii_case("filename") { &mut plain }
                else if key.eq_ignore_ascii_case("filename*") { &mut extended }
                else { continue };
            if slot.replace(value).is_some() { parsed.ambiguous = true; }
        }
        let name = name.ok_or(Error::Malformed)?.text();
        let content = &body[content_start..end];
        match filename(extended.map(Value::text).as_deref(), plain.map(Value::text).as_deref()) {
            Some(filename) if name == "file" => { parsed.files += 1; if !filename.is_empty() { parsed.file = Some((content.to_vec(), filename)) } }
            Some(_) => parsed.file_parts.push((name, String::from_utf8_lossy(content).into_owned())),
            None => parsed.fields.push((name, String::from_utf8_lossy(content).into_owned())),
        }
        (closing, at) = (close, next);
    }
}

/// `filename*` (RFC 5987: `charset'language'percent-encoded`) over `filename`; `filename` with a
/// quoted string's backslash escapes undone, and percent-decoded when every `%` begins an escape (as
/// Rack reads what browsers percent-encode).
fn filename(extended: Option<&str>, plain: Option<&str>) -> Option<String> {
    if let Some(value) = extended.and_then(|v| v.splitn(3, '\'').nth(2)) {
        return Some(percent_decode(value));
    }
    let value = plain?;
    let escapes = value.match_indices('%').all(|(i, _)| value.get(i + 1..i + 3).is_some_and(|h| h.bytes().all(|b| b.is_ascii_hexdigit())));
    Some(if escapes { percent_decode(value) } else { value.to_string() })
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if let (b'%', Some(Some(h)), Some(Some(l))) = (bytes[i], bytes.get(i + 1).map(|b| hex(*b)), bytes.get(i + 2).map(|b| hex(*b))) {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A parameter's value as found: unquoted (trimmed when read), or a quoted string's inside (its `\\x` read as `x`).
#[derive(Clone, Copy)]
enum Value<'a> { Plain(&'a str), Quoted(&'a str) }

impl Value<'_> {
    fn text(self) -> String {
        match self {
            Value::Plain(v) => v.trim().to_string(),
            Value::Quoted(v) => {
                let mut out = String::with_capacity(v.len());
                let mut chars = v.chars();
                while let Some(c) = chars.next() { if c == '\\' { out.extend(chars.next()) } else { out.push(c) } }
                out
            }
        }
    }
}

/// `; key=value; key="quoted; value"` parameters, one at a time, each key trimmed and each value as found: nothing is
/// copied until a caller reads a value it keeps.
fn params(text: &str) -> impl Iterator<Item = (&str, Value<'_>)> {
    let mut chars = text.char_indices().peekable();
    let end = text.len();
    std::iter::from_fn(move || {
        while chars.next_if(|(_, c)| *c == ';' || c.is_whitespace()).is_some() {}
        let at = |chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>| chars.peek().map_or(end, |(i, _)| *i);
        let start = at(&mut chars);
        while chars.next_if(|(_, c)| *c != '=' && *c != ';').is_some() {}
        let key = &text[start..at(&mut chars)];
        if key.is_empty() && chars.peek().is_none() { return None; }
        let mut value = Value::Plain("");
        if chars.next_if(|(_, c)| *c == '=').is_some() {
            if chars.next_if(|(_, c)| *c == '"').is_some() {
                let from = at(&mut chars);
                let mut to = end;
                while let Some((i, c)) = chars.next() {
                    match c { '"' => { to = i; break } '\\' => { chars.next(); } _ => {} }
                }
                value = Value::Quoted(&text[from..to]);
                while chars.next_if(|(_, c)| *c != ';').is_some() {}
            } else {
                let from = at(&mut chars);
                while chars.next_if(|(_, c)| *c != ';').is_some() {}
                value = Value::Plain(&text[from..at(&mut chars)]);
            }
        }
        Some((key.trim(), value))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const B: &str = "----b0undary";

    fn body(parts: &[&str]) -> Vec<u8> {
        let mut out = String::new();
        for part in parts { out.push_str(&format!("--{B}\r\n{part}\r\n")); }
        out.push_str(&format!("--{B}--\r\n"));
        out.into_bytes()
    }
    fn file(filename: &str, content: &str) -> String {
        format!("Content-Disposition: form-data; name=\"file\"; {filename}\r\nContent-Type: text/csv\r\n\r\n{content}")
    }

    #[test]
    fn boundary_is_read_plain_or_quoted() {
        assert_eq!(boundary("multipart/form-data; boundary=----x").as_deref(), Some("----x"));
        assert_eq!(boundary("Multipart/Form-Data; charset=utf-8; boundary=\"a b\"").as_deref(), Some("a b"));
        assert_eq!(boundary("multipart/form-data"), None);
        assert_eq!(boundary("multipart/mixed; boundary=x"), None);
        assert_eq!(boundary("multipart/form-data; boundary="), None);
    }

    #[test]
    fn text_fields_and_the_file_bytes_kept_raw_bom_and_crlf_included() {
        let csv = "\u{feff}\"User_ID\",\"UTC_Time\"\r\n\"1\",\"2024-01-01 00:00:00\"\r\n";
        let parsed = parse(&body(&["Content-Disposition: form-data; name=\"authenticity_token\"\r\n\r\ntok",
            "Content-Disposition: form-data; name=\"format\"\r\n\r\nbinance",
            &file("filename=\"Binance (UTC+3).csv\"", csv)]), B).unwrap();
        assert_eq!(parsed.fields, vec![("authenticity_token".into(), "tok".into()), ("format".into(), "binance".into())]);
        let (bytes, name) = parsed.file.unwrap();
        assert_eq!(bytes, csv.as_bytes(), "BOM and CRLF untouched");
        assert!(bytes.starts_with(&[0xef, 0xbb, 0xbf]));
        assert_eq!(name, "Binance (UTC+3).csv");
    }

    #[test]
    fn invalid_utf8_and_a_lone_crlf_dash_in_the_file_survive() {
        let mut raw = format!("--{B}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.csv\"\r\n\r\n").into_bytes();
        raw.extend_from_slice(b"\xff\xfe\r\n--not-the-boundary\r\n");
        raw.extend_from_slice(format!("\r\n--{B}--").as_bytes());
        assert_eq!(parse(&raw, B).unwrap().file.unwrap().0, b"\xff\xfe\r\n--not-the-boundary\r\n");
    }

    #[test]
    fn filenames_quoted_escaped_percent_encoded_and_extended() {
        let name = |disposition: &str| parse(&body(&[&file(disposition, "x")]), B).unwrap().file.unwrap().1;
        assert_eq!(name("filename=\"Export; 2024 (UTC+3).csv\""), "Export; 2024 (UTC+3).csv");
        assert_eq!(name("filename=\"a \\\"q\\\" (UTC-5:30).csv\""), "a \"q\" (UTC-5:30).csv");
        assert_eq!(name("filename=\"a%22b%20(UTC+3).csv\""), "a\"b (UTC+3).csv");
        assert_eq!(name("filename=\"100% (UTC+3).csv\""), "100% (UTC+3).csv");
        assert_eq!(name("filename=plain.csv"), "plain.csv");
        assert_eq!(name("filename=\"fallback.csv\"; filename*=UTF-8''%C5%BC%20(UTC%2B3).csv"), "ż (UTC+3).csv");
    }

    #[test]
    fn a_missing_or_blank_file_part_is_no_file() {
        let fields = "Content-Disposition: form-data; name=\"format\"\r\n\r\ndeltabadger";
        assert_eq!(parse(&body(&[fields]), B).unwrap().file, None);
        let blank = "Content-Disposition: form-data; name=\"file\"; filename=\"\"\r\nContent-Type: application/octet-stream\r\n\r\n";
        let parsed = parse(&body(&[fields, blank]), B).unwrap();
        assert_eq!((parsed.file, parsed.fields.len()), (None, 1));
        assert_eq!(parse(format!("--{B}--\r\n").as_bytes(), B).unwrap(), Parsed::default());
    }

    #[test]
    fn a_truncated_or_broken_body_is_malformed() {
        let whole = body(&[&file("filename=\"a.csv\"", "a,b\r\n1,2")]);
        let cut = format!("\r\n--{B}--\r\n").len() + 3;
        assert_eq!(parse(&whole[..whole.len() - cut], B), Err(Error::Malformed), "no closing delimiter");
        assert_eq!(parse(&whole[..20], B), Err(Error::Malformed), "inside the headers");
        assert_eq!(parse(b"", B), Err(Error::Malformed));
        assert_eq!(parse(&whole, "other"), Err(Error::Malformed));
        assert_eq!(parse(&body(&["Content-Type: text/plain\r\n\r\nx"]), B), Err(Error::Malformed), "no disposition");
        assert_eq!(parse(&body(&["Content-Disposition: form-data\r\n\r\nx"]), B), Err(Error::Malformed), "no name");
    }

    /// PLAN-710-importmem C1: the last `name`, `filename` and `filename*` each, as before; a valid `filename*` over any
    /// `filename`; only the outer `boundary` is the first.
    #[test]
    fn the_last_of_each_parameter_is_read() {
        let part = |disposition: &str| parse(&body(&[&format!("Content-Disposition: form-data; {disposition}\r\n\r\nx")]), B).unwrap();
        let named_token = part("name=\"file\"; name=\"token\"; filename=\"a.csv\"");
        assert_eq!((named_token.file, named_token.file_parts), (None, vec![("token".to_string(), "x".to_string())]), "the last name: no file");
        let zone = part("name=\"file\"; filename*=UTF-8''Binance%20(UTC%2B3).csv; filename=\"plain.csv\"").file.unwrap().1;
        assert_eq!(zone, "Binance (UTC+3).csv", "filename* over filename, whatever their order");
        assert_eq!(part("name=\"file\"; filename=\"a.csv\"; filename=\"b.csv\"").file.unwrap().1, "b.csv");
        assert_eq!(part("name=\"file\"; filename*=UTF-8''a.csv; filename*=UTF-8''b.csv").file.unwrap().1, "b.csv");
        assert_eq!(part("NAME=\"format\"; x=\"a\\\"; name=y\"").fields, vec![("format".to_string(), "x".to_string())], "an escaped quote stays inside");
        assert_eq!(boundary("multipart/form-data; boundary=a; boundary=b").as_deref(), Some("a"));
    }

    /// D-MULTIPART-DELIMITER (PLAN-710-mcpadapter G3/H4): a delimiter is a whole line: `--boundary`, then `--` or nothing,
    /// optional linear whitespace, then CRLF (or the body's end after the closing one). Anything else on the line is the
    /// part's content, so a file is never cut short at a line that only starts like one.
    #[test]
    fn a_delimiter_is_a_whole_line() {
        let csv = format!("a,b\r\n--{B}--x\r\n1,2\r\n--{B} junk\r\n3,4");
        assert_eq!(parse(&body(&[&file("filename=\"a.csv\"", &csv)]), B).unwrap().file.unwrap().0, csv.as_bytes(), "every row kept");
        let mut closed = format!("--{B}\r\n{}\r\n--{B}-- \t", file("filename=\"a.csv\"", "x")).into_bytes();
        assert_eq!(parse(&closed, B).unwrap().file.unwrap().0, b"x", "the closing delimiter, blanks, then the end");
        closed.extend_from_slice(b"\r\nepilogue");
        assert_eq!(parse(&closed, B).unwrap().file.unwrap().0, b"x", "an epilogue after it");
        let open = format!("--{B}\r\n{}\r\n--{B}--x", file("filename=\"a.csv\"", "x"));
        assert_eq!(parse(open.as_bytes(), B), Err(Error::Malformed), "no closing delimiter: the file is never kept in part");
    }

    /// PLAN-710-mcpadapter I3/J2/J7: a part read two ways is reported, never decided: two dispositions, two names, two
    /// `filename`s or two `filename*`s. The reader still reads as before (the hook relies on it); the import callers refuse.
    #[test]
    fn a_part_read_two_ways_is_reported() {
        let part = |head: &str| parse(&body(&[&format!("{head}\r\n\r\nx")]), B).unwrap();
        for head in ["Content-Disposition: form-data; name=\"format\"\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.csv\"",
                     "Content-Disposition: form-data; name=\"file\"; name=\"token\"; filename=\"a.csv\"",
                     "Content-Disposition: form-data; name=\"file\"; filename=\"a.csv\"; filename=\"b.csv\"",
                     "Content-Disposition: form-data; name=\"file\"; filename*=UTF-8''a.csv; filename*=UTF-8''b.csv"] {
            assert!(part(head).ambiguous, "{head}");
        }
        let both = part("Content-Disposition: form-data; name=\"file\"; filename=\"plain.csv\"; filename*=UTF-8''z.csv");
        assert!(!both.ambiguous);
        assert_eq!((both.file.unwrap().1, both.files), ("z.csv".to_string(), 1));
        let twice = parse(&body(&[&file("filename=\"a.csv\"", "x"), &file("filename=\"b.csv\"", "y")]), B).unwrap();
        assert_eq!((twice.files, twice.ambiguous), (2, false), "two file parts are counted; which import caller refuses them is its own");
        assert_eq!(part("Content-Disposition: form-data; name=\"file\"; name=\"token\"; filename=\"a.csv\"").file_parts, vec![("token".to_string(), "x".to_string())], "still the last name");
    }

    #[test]
    fn the_part_count_is_capped() {
        let field = "Content-Disposition: form-data; name=\"x\"\r\n\r\n1";
        assert!(parse(&body(&vec![field; PARTS]), B).is_ok());
        assert_eq!(parse(&body(&vec![field; PARTS + 1]), B), Err(Error::TooManyParts));
    }
}
