use super::WebError;
use rusqlite::Connection;
pub fn supported(c:&Connection,owner:i64)->Result<bool,WebError>{
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM api_keys WHERE user_id=?1)
    AND NOT EXISTS(SELECT 1 FROM api_keys k LEFT JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 AND (e.type IS NULL OR e.type!='Exchanges::Alpaca' OR COALESCE(k.status,-1)!=1 OR COALESCE(k.key_type,-1) NOT IN(0,2) OR COALESCE(k.last_sync_error,'')!=''))
    AND (SELECT COUNT(DISTINCT exchange_id) FROM api_keys WHERE user_id=?1)=1
    AND NOT EXISTS(SELECT 1 FROM account_transactions WHERE user_id=?1)
    AND NOT EXISTS(SELECT 1 FROM portfolio_snapshots WHERE user_id=?1)
    AND NOT EXISTS(SELECT 1 FROM portfolio_venue_snapshots WHERE user_id=?1)
    AND NOT EXISTS(SELECT 1 FROM app_configs WHERE key=?2)
    AND EXISTS(SELECT 1 FROM account_balances WHERE user_id=?1)
    AND NOT EXISTS(SELECT 1 FROM account_balances WHERE user_id=?1 AND exchange_id NOT IN(SELECT exchange_id FROM api_keys WHERE user_id=?1))",rusqlite::params![owner,crate::tracker::cache::key(owner)],|r|r.get(0))?)
}
