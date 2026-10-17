//! Import::DeltabadgerCsv: the app's own export (the columns web::tracker::CSV_HEADERS writes) read back. Every row
//! names its venue; nothing here is stripped but the type and the venue, and a description keeps CsvSafe's leading
//! quote, as Rails reads it. Pinned by rust/tests/fixtures/import_vectors.json.
use super::csv::{self, Malformed, Table};
use crate::ruby;
use crate::sync::activities::{Entry, Raw};
use crate::web::tracker::CSV_HEADERS;
use serde_json::json;

/// One entry and the venue (`exchange_name_id`) its row names.
#[derive(Debug, Clone, PartialEq)]
pub struct Routed { pub exchange: Option<String>, pub entry: Entry }

/// Import::DeltabadgerCsv.new(text).entries, oldest first (stable), with #unrecognised. `text` is Import::Run.as_text's.
pub fn read(text: &str) -> Result<(Vec<Routed>, Vec<String>), Malformed> {
    let [date, kind, base_currency, base_amount, quote_currency, quote_amount, fee_currency, fee_amount, exchange, tx_id, group_id, description] = CSV_HEADERS;
    let table = Table::parse(text)?;
    let (mut out, mut unrecognised) = (vec![], Vec::<String>::new());
    for row in &table.rows {
        let get = |name: &str| table.get(row, name);
        let presence = |name: &str| get(name).filter(|v| !ruby::blank(v)).map(str::to_string);
        let type_name = ruby::strip(get(kind).unwrap_or(""));
        let Some(entry_type) = super::entry_type(type_name) else {
            if !ruby::blank(type_name) && !unrecognised.iter().any(|u| u == type_name) { unrecognised.push(type_name.to_string()); }
            continue;
        };
        let amount = |name: &str| presence(name).map(|v| csv::amount(Some(&v))).transpose();
        let exchange = Some(ruby::strip(get(exchange).unwrap_or(""))).filter(|v| !ruby::blank(v)).map(str::to_string);
        let entry = Entry {
            entry_type,
            base_currency: get(base_currency).map(str::to_string),
            base_amount: csv::amount(get(base_amount))?,
            quote_currency: presence(quote_currency),
            quote_amount: amount(quote_amount)?,
            fee_currency: presence(fee_currency),
            fee_amount: amount(fee_amount)?,
            tx_id: presence(tx_id), group_id: presence(group_id), description: presence(description),
            transacted_at: Some(csv::time(get(date), None)?),
            raw: Raw::from_value(json!({})),
        };
        out.push(Routed { exchange, entry });
    }
    out.sort_by_key(|r| r.entry.transacted_at);
    Ok((out, unrecognised))
}
