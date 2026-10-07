//! Legacy display currencies normalize at every reader, without rewriting the stored row.
mod common;
use common::web::{self,TestClock};
use deltabadger::{app_config,web::{auth,tracker::fx}};
use serde_json::json;

#[tokio::test(flavor="current_thread")]
async fn legacy_currency_is_normalized_for_current_fx_without_writes() {
    let (dir,opened,seed)=common::install_alpaca();let c=opened.primary;
    for(k,v) in [("market_data_provider","deltabadger"),("market_data_url","http://127.0.0.1:1"),("market_data_token","synthetic-index-token")] {
        app_config::set_plain(&c,k,v,web::at("2026-01-01T00:00:00Z")).unwrap();
    }
    let app=web::app(dir.path(),web::SECRET,TestClock::at("2026-09-10T12:00:30Z"))
        .with_figure_source(deltabadger::web::figure::loading::Source::Script(json!({
            "GET 127.0.0.1:1/api/v1/exchange_rates":{"body":{"data":{"usd":{"value":100.0},"eur":{"value":80.0}}}}
        }))).unwrap();
    for(raw,wanted,rate) in [("usd","USD","1.0"),("eur","EUR","0.8"),("","USD","1.0"),(" \t\n","USD","1.0"),("\u{3000}","USD","1.0")] {
        c.execute("UPDATE users SET display_currency=?1 WHERE id=?2",(raw,seed.user_id)).unwrap();
        let before:String=c.query_row("SELECT updated_at FROM users WHERE id=?1",[seed.user_id],|r|r.get(0)).unwrap();
        assert_eq!(auth::User::find(&c,seed.user_id).unwrap().unwrap().display_currency,raw,"auth preserves stored preference {raw:?}");
        assert_eq!(deltabadger::figures::db::user(&c,seed.user_id).unwrap().display_currency,raw,"figure input {raw:?}");
        let prepared=fx::prepare(&app,seed.user_id).await.unwrap();
        deltabadger::figures::budget::within(|| {
            let d=prepared.denomination(&c,seed.user_id,app.now().timestamp()).unwrap().expect("normalized FX available");
            assert_eq!(d.currency,wanted);assert_eq!(d.rate.to_s_f(),rate);Ok::<_,deltabadger::figures::FiguresError>(())
        }).unwrap();
        assert_eq!(c.query_row("SELECT display_currency,updated_at FROM users WHERE id=?1",[seed.user_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).unwrap(),(raw.to_string(),before));
    }
}

#[tokio::test(flavor="current_thread")]
async fn normalized_currency_changes_still_invalidate_prepared_fx() {
    let(dir,opened,seed)=common::install_alpaca();let c=opened.primary;
    c.execute("UPDATE users SET display_currency='usd'",[]).unwrap();
    let app=web::app(dir.path(),web::SECRET,TestClock::at("2026-09-10T12:00:30Z"));
    let prepared=fx::prepare(&app,seed.user_id).await.unwrap();
    c.execute("UPDATE users SET display_currency='eur'",[]).unwrap();
    assert!(prepared.denomination(&c,seed.user_id,app.now().timestamp()).unwrap().is_none());
}
