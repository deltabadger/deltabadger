//! The populated first-sync state. Every later account state stays explicitly deferred.
use crate::web::WebError;
use rusqlite::Connection;

/// A complete first-sync page only: no history can be mislabeled as an empty record.
/// Keep the owner and the absence predicates in SQL, including zero balances.
pub fn supported(c:&Connection,owner:i64)->Result<bool,WebError>{
    Ok(c.query_row("SELECT
      EXISTS(SELECT 1 FROM api_keys WHERE user_id=?1)
      AND NOT EXISTS(SELECT 1 FROM api_keys k LEFT JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 AND (e.type IS NULL OR e.type!='Exchanges::Alpaca' OR COALESCE(k.status,-1)!=1 OR COALESCE(k.key_type,-1) NOT IN (0,2) OR k.last_synced_at IS NOT NULL OR k.balances_synced_at IS NOT NULL OR COALESCE(k.last_sync_error,'')!=''))
      AND NOT EXISTS(SELECT 1 FROM account_transactions WHERE user_id=?1)
      AND NOT EXISTS(SELECT 1 FROM account_balances WHERE user_id=?1)
      AND NOT EXISTS(SELECT 1 FROM portfolio_snapshots WHERE user_id=?1)
      AND NOT EXISTS(SELECT 1 FROM portfolio_venue_snapshots WHERE user_id=?1)",[owner],|r|r.get(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn db()->Connection {
        let c=Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE exchanges(id INTEGER,type TEXT); CREATE TABLE api_keys(user_id INTEGER,exchange_id INTEGER,status INTEGER,key_type INTEGER,last_synced_at TEXT,balances_synced_at TEXT,last_sync_error TEXT); CREATE TABLE account_transactions(user_id INTEGER); CREATE TABLE account_balances(user_id INTEGER); CREATE TABLE portfolio_snapshots(user_id INTEGER); CREATE TABLE portfolio_venue_snapshots(user_id INTEGER); INSERT INTO exchanges VALUES(1,'Exchanges::Alpaca'),(2,'Exchanges::Kraken'); INSERT INTO api_keys VALUES(7,1,1,0,NULL,NULL,NULL);").unwrap();
        c
    }
    #[test]
    fn first_sync_reads_only_the_owner_and_requires_an_unused_reading_alpaca_key() {
        let c=db();
        assert!(super::super::read::only(&c,|c|supported(c,7)).unwrap());
        assert!(!supported(&c,8).unwrap());
        for table in ["account_transactions","account_balances","portfolio_snapshots","portfolio_venue_snapshots"] {
            c.execute(&format!("INSERT INTO {table} VALUES(8)"),[]).unwrap();
            assert!(supported(&c,7).unwrap(),"foreign {table}");
            c.execute(&format!("INSERT INTO {table} VALUES(7)"),[]).unwrap();
            assert!(!supported(&c,7).unwrap(),"owned {table}");
            c.execute(&format!("DELETE FROM {table}"),[]).unwrap();
        }
        for change in ["status=0","status=2","status=NULL","key_type=1","key_type=NULL","last_synced_at='2026-01-01'","balances_synced_at='2026-01-01'","last_sync_error='failed'","exchange_id=2"] {
            let c=db();c.execute(&format!("UPDATE api_keys SET {change}"),[]).unwrap();
            assert!(!supported(&c,7).unwrap(),"{change}");
        }
        let c=db();c.execute("UPDATE api_keys SET key_type=2",[]).unwrap();assert!(supported(&c,7).unwrap());
        c.execute("INSERT INTO api_keys VALUES(8,2,2,1,'old','old','foreign')",[]).unwrap();assert!(supported(&c,7).unwrap());
        c.execute("INSERT INTO api_keys VALUES(7,2,1,0,NULL,NULL,NULL)",[]).unwrap();assert!(!supported(&c,7).unwrap());
    }
    #[test]
    fn first_sync_database_errors_are_propagated() {
        let c=db();c.execute("DROP TABLE account_balances",[]).unwrap();
        assert!(supported(&c,7).is_err());
    }
}
