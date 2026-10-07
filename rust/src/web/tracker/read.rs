//! Tracker view work runs with SQLite writes disabled. Never schedules work from a GET.
use crate::web::WebError;
use rusqlite::Connection;

pub fn only<T>(c:&Connection,read:impl FnOnce(&Connection)->Result<T,WebError>)->Result<T,WebError>{
    let was:i64=c.query_row("PRAGMA query_only",[],|r|r.get(0))?;
    c.pragma_update(None,"query_only",true)?;
    let result=read(c);
    // Always restore, including a failing view. Propagate a restoration failure as well.
    c.pragma_update(None,"query_only",was)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn view_boundary_rejects_writes_and_restores_connection_on_error() {
        let c=Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE rows(id INTEGER,updated_at TEXT); INSERT INTO rows VALUES(1,'before');").unwrap();
        for sql in ["UPDATE rows SET updated_at='after'","DELETE FROM rows","INSERT INTO rows VALUES(2,'new')","CREATE TABLE surprise(id)"] {
            let result=only(&c,|c|{c.execute_batch(sql)?;Ok(())});assert!(result.is_err(),"view wrote: {sql}");
            assert_eq!(c.query_row("SELECT COUNT(*) FROM rows WHERE updated_at='before'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
            assert_eq!(c.query_row("PRAGMA query_only",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        }
        c.execute("UPDATE rows SET updated_at='writer'",[]).unwrap();
        assert_eq!(only(&c,|c|Ok(c.query_row("SELECT updated_at FROM rows",[],|r|r.get::<_,String>(0))?)).unwrap(),"writer");
        c.pragma_update(None,"query_only",true).unwrap();
        assert_eq!(only(&c,|c|Ok(c.query_row("SELECT COUNT(*) FROM rows",[],|r|r.get::<_,i64>(0))?)).unwrap(),1);
        assert_eq!(c.query_row("PRAGMA query_only",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    }
}
