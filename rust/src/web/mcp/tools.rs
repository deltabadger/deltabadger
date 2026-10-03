use super::protocol::tool_text;
use crate::web::{bearer::Bearer, consent, WebError};
use rusqlite::Connection;
use serde_json::Value;
pub const NAMES: [&str;4] = ["list_bots","list_exchanges","list_transactions","list_tax_jurisdictions"];
// No tools are registered until Task 4 ports their implementations.
pub fn registry(_c: &Connection, _who: Bearer) -> Result<Vec<String>,WebError> { Ok(vec![]) }
pub fn gate(c: &Connection, who: Bearer, name: &str) -> Result<Option<Value>,WebError> {
    let (enabled,granted) = consent::mcp_access(c,who.user_id,who.application_id)?;
    Ok(if !enabled.iter().any(|n| n == name) {Some(tool_text(&format!("Tool '{name}' is disabled. Enable it in Settings > MCP."),true))}
       else if !granted.iter().any(|n| n == name) {Some(tool_text(&format!("Tool '{name}' is not available to this client. Grant it in Settings > Connect."),true))} else {None})
}
