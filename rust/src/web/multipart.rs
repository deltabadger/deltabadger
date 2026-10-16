//! `multipart/form-data` (RFC 7578), read by `entry` for the one route that takes a file:
//! POST /tracker/import. Text parts become form fields; the part named `file` is kept as raw bytes
//! with its filename, as Rails' `params[:file].read.b` and `original_filename`.

/// The most parts one body may hold. The import form posts four.
pub const PARTS: usize = 64;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub fields: Vec<(String, String)>,
    /// The `file` part's bytes and filename. A part whose filename is empty (a file input left
    /// empty) is no file, as Rack drops it.
    pub file: Option<(Vec<u8>, String)>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error { Malformed, TooManyParts }

/// The `boundary` parameter of a `multipart/form-data` content type.
pub fn boundary(content_type: &str) -> Option<String> {
    let (media, rest) = content_type.split_once(';')?;
    if !media.trim().eq_ignore_ascii_case("multipart/form-data") { return None; }
    params(rest).into_iter().find(|(k, _)| k.eq_ignore_ascii_case("boundary")).map(|(_, v)| v)
        .filter(|b| !b.is_empty() && b.len() <= 70)
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|i| i + from)
}

pub fn parse(body: &[u8], boundary: &str) -> Result<Parsed, Error> {
    let delimiter = format!("--{boundary}");
    let between = format!("\r\n--{boundary}");
    // Anything before the first delimiter is a preamble and ignored.
    let mut at = if body.starts_with(delimiter.as_bytes()) { delimiter.len() }
        else { find(body, between.as_bytes(), 0).ok_or(Error::Malformed)? + between.len() };
    let mut parsed = Parsed::default();
    let mut parts = 0;
    loop {
        if body[at..].starts_with(b"--") { return Ok(parsed); }
        while matches!(body.get(at), Some(b' ' | b'\t')) { at += 1; }
        if !body[at..].starts_with(b"\r\n") { return Err(Error::Malformed); }
        at += 2;
        parts += 1;
        if parts > PARTS { return Err(Error::TooManyParts); }
        if body[at..].starts_with(b"\r\n") { return Err(Error::Malformed); } // a part with no headers names nothing
        let head_end = find(body, b"\r\n\r\n", at).ok_or(Error::Malformed)?;
        let content_start = head_end + 4;
        let end = find(body, between.as_bytes(), content_start).ok_or(Error::Malformed)?;
        let head = std::str::from_utf8(&body[at..head_end]).map_err(|_| Error::Malformed)?;
        let disposition = head.split("\r\n")
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-disposition"))
            .map(|(_, value)| value).ok_or(Error::Malformed)?;
        let (kind, rest) = disposition.split_once(';').unwrap_or((disposition, ""));
        if !kind.trim().eq_ignore_ascii_case("form-data") { return Err(Error::Malformed); }
        let params = params(rest);
        let get = |key: &str| params.iter().rfind(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str());
        let name = get("name").ok_or(Error::Malformed)?.to_string();
        let content = &body[content_start..end];
        match filename(get("filename*"), get("filename")) {
            Some(filename) if name == "file" => if !filename.is_empty() { parsed.file = Some((content.to_vec(), filename)) },
            Some(_) => {}
            None => parsed.fields.push((name, String::from_utf8_lossy(content).into_owned())),
        }
        at = end + between.len();
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

/// `; key=value; key="quoted; value"` parameters, a quoted value's `\x` read as `x`.
fn params(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.next_if(|c| *c == ';' || c.is_whitespace()).is_some() {}
        let key: String = std::iter::from_fn(|| chars.next_if(|c| *c != '=' && *c != ';')).collect();
        if key.is_empty() && chars.peek().is_none() { return out; }
        let mut value = String::new();
        if chars.next_if_eq(&'=').is_some() {
            if chars.next_if_eq(&'"').is_some() {
                while let Some(c) = chars.next() {
                    match c { '"' => break, '\\' => value.extend(chars.next()), c => value.push(c) }
                }
                while chars.next_if(|c| *c != ';').is_some() {}
            } else {
                value = std::iter::from_fn(|| chars.next_if(|c| *c != ';')).collect::<String>().trim().to_string();
            }
        }
        out.push((key.trim().to_string(), value));
    }
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

    #[test]
    fn the_part_count_is_capped() {
        let field = "Content-Disposition: form-data; name=\"x\"\r\n\r\n1";
        assert!(parse(&body(&vec![field; PARTS]), B).is_ok());
        assert_eq!(parse(&body(&vec![field; PARTS + 1]), B), Err(Error::TooManyParts));
    }
}
