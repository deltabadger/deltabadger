//! Import::DeltabadgerCsv: the app's own export (the columns web::tracker::CSV_HEADERS writes) read back. Every row
//! names its venue; nothing here is stripped but the type and the venue. D-CSV-ROUNDTRIP: a text cell loses the quote
//! CsvSafe put ahead of a formula character (CsvSafe.unescape), so a round trip is lossless; 2.x keeps it. Pinned by
//! rust/tests/fixtures/import_vectors.json.
use super::csv::{self, Malformed, Table};
use crate::v2;
use crate::sync::activities::{Entry, Raw};
use crate::web::tracker::CSV_HEADERS;
use serde_json::json;

/// One entry, the venue (`exchange_name_id`) its row names and its account number there (#710: absent or blank, 1).
#[derive(Debug, Clone, PartialEq)]
pub struct Routed { pub exchange: Option<String>, pub account: u32, pub entry: Entry }

/// CsvSafe.unescape: drop the one leading `'` CsvSafe.cell adds, only when what follows reads as a formula.
pub(crate) fn unescape(v: &str) -> &str {
    const LEADERS: [char; 7] = ['=', '+', '-', '@', '\t', '\r', '\n'];
    v.strip_prefix('\'').filter(|rest| rest.starts_with(LEADERS) || v2::strip(rest).starts_with(LEADERS)).unwrap_or(v)
}

/// Import::DeltabadgerCsv.new(text).entries, oldest first (stable), with #unrecognised. `text` is Import::Run.as_text's.
pub fn read(text: &str) -> Result<(Vec<Routed>, Vec<String>), Malformed> {
    let [date, kind, base_currency, base_amount, quote_currency, quote_amount, fee_currency, fee_amount, exchange, tx_id, group_id, description, account] = CSV_HEADERS;
    let table = Table::parse(text)?;
    let (mut out, mut unrecognised) = (vec![], Vec::<String>::new());
    for row in &table.rows {
        let get = |name: &str| table.get(row, name);
        // The text columns only: amounts, the date and the type are read as 2.x reads them.
        let text = |name: &str| get(name).map(unescape);
        let presence = |name: &str| text(name).filter(|v| !v2::blank(v)).map(str::to_string);
        let type_name = v2::strip(get(kind).unwrap_or(""));
        let Some(entry_type) = super::entry_type(type_name) else {
            if !v2::blank(type_name) && !unrecognised.iter().any(|u| u == type_name) { unrecognised.push(type_name.to_string()); }
            continue;
        };
        let amount = |name: &str| get(name).filter(|v| !v2::blank(v)).map(|v| csv::amount(Some(v))).transpose();
        let exchange = Some(v2::strip(text(exchange).unwrap_or(""))).filter(|v| !v2::blank(v)).map(str::to_string);
        // D-CSV-ACCOUNT-COLUMN: a positive number; a file from before it (or a blank cell) names number 1.
        let account = match get(account).map(v2::strip).filter(|v| !v2::blank(v)) {
            None => 1,
            Some(n) => n.parse::<u32>().ok().filter(|n| *n > 0).ok_or_else(|| Malformed(format!("account {n}")))?,
        };
        let entry = Entry {
            entry_type,
            base_currency: text(base_currency).map(str::to_string),
            base_amount: csv::amount(get(base_amount))?,
            quote_currency: presence(quote_currency),
            quote_amount: amount(quote_amount)?,
            fee_currency: presence(fee_currency),
            fee_amount: amount(fee_amount)?,
            tx_id: presence(tx_id), group_id: presence(group_id), description: presence(description),
            transacted_at: Some(csv::time(get(date), None)?),
            raw: Raw::from_value(json!({})),
        };
        out.push(Routed { exchange, account, entry });
    }
    out.sort_by_key(|r| r.entry.transacted_at);
    Ok((out, unrecognised))
}
