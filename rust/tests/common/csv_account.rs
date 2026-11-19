//! D-CSV-ACCOUNT-COLUMN (#710, PLAN-710-4 §5): the tracker CSV has a 13th column, `account`, the row's account number on
//! its exchange. Every recorded 2.x install has one account per exchange, so 2.x's answer is read with `account` 1 on
//! every row: the recorded files stay as recorded, and everything else of the CSV stays compared.
use serde_json::Value;

const HEAD: &str = "date,type,base_currency,base_amount,quote_currency,quote_amount,fee_currency,fee_amount,exchange,tx_id,group_id,description";

/// A 2.x tracker CSV with the `account` column added (`,account` on the header, `,1` on each record), or None for any
/// other text. Records end at a line break outside quotes.
pub fn add(text: &str) -> Option<String> {
    if !text.starts_with(HEAD) || text.starts_with(&format!("{HEAD},")) { return None; }
    let (mut out, mut quoted, mut first) = (String::new(), false, true);
    let mut chars = text.chars().peekable();
    let mut end = |out: &mut String| { out.push_str(if first { ",account" } else { ",1" }); first = false; };
    while let Some(ch) = chars.next() {
        if ch == '"' { quoted = !quoted; }
        if !quoted && (ch == '\n' || ch == '\r') {
            end(&mut out);
            out.push(ch);
            if ch == '\r' && chars.peek() == Some(&'\n') { out.push(chars.next().unwrap()); }
            continue;
        }
        out.push(ch);
    }
    if !text.ends_with(['\n', '\r']) { end(&mut out); }
    Some(out)
}

/// `add` over every string of `v`, a JSON text's strings included (an MCP body, a stored session message). The count changed.
pub fn walk(v: &mut Value) -> usize {
    match v {
        Value::String(s) => {
            if let Some(added) = add(s) { *s = added; return 1; }
            if !s.contains(HEAD) { return 0; }
            let Ok(mut inner) = serde_json::from_str::<Value>(s) else { return 0 };
            let n = walk(&mut inner);
            if n > 0 { *s = serde_json::to_string(&inner).unwrap(); }
            n
        }
        Value::Array(items) => items.iter_mut().map(walk).sum(),
        Value::Object(map) => map.values_mut().map(walk).sum(),
        _ => 0,
    }
}
