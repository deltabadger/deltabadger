//! The IBKR connect page on Rust (Ibkr plan I3) where it differs from Rails by design, beside its page parity
//! (pages_settings.rb settings_ibkr_*): a new connection's keys made inline with ffdhe2048 (D-IBKR-DHGROUP), downloads of
//! their public halves, a key whose material makes no session offering Disconnect only (D-IBKR-RECONNECT), and a
//! disconnect that drops the key's account binding and keeps its ledger rows.
mod common;
#[path = "support/settings_fixture.rs"]
mod fixture;
use common::web::{self, Browser, Csrf};
use deltabadger::venue::signers::ibkr as signer;
use deltabadger::web::{App, Config};

const NOW: &str = "2026-09-10T12:00:30.123456Z";

struct Fx { _dir: tempfile::TempDir, c: rusqlite::Connection, app: App, browser: Browser, user: i64, exchange: i64 }

async fn hosted() -> Fx {
    let (dir, opened, seed) = fixture::install();
    deltabadger::store::test_support::migrate(&opened.primary).unwrap();
    let env = |name: &str| match name { "SECRET_KEY_BASE" => Some(web::SECRET.to_string()), "MARKET_DATA_URL" => Some("http://data-api.invalid".into()), _ => None };
    let app = App::new(Config::from_env(&env).unwrap(), &env, opened.primary, web::TestClock::at(NOW)).unwrap()
        .with_figure_source(deltabadger::web::figure::loading::Source::Disabled).unwrap();
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    c.execute("UPDATE exchanges SET type = 'Exchanges::Ibkr', name = 'Interactive Brokers' WHERE id = ?1", [seed.exchange_id]).unwrap();
    c.execute("DELETE FROM api_keys", []).unwrap();
    c.execute("UPDATE users SET encrypted_password=?1,confirmed_at='2026-01-01 00:00:00',setup_completed=1 WHERE id=?2",
              (deltabadger::crypto::hash_password("Correct-horse-9").unwrap(), seed.user_id)).unwrap();
    let mut browser = Browser::default();
    browser.send(&app, "GET", "/login", None, Csrf::None, &[]).await;
    let signed = browser.send(&app, "POST", "/login", Some(&[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]), Csrf::Form, &[]).await;
    assert_eq!(signed.status, 303);
    Fx { _dir: dir, c, app, browser, user: seed.user_id, exchange: seed.exchange_id }
}

impl Fx {
    async fn page(&mut self) -> String {
        let answer = self.browser.send(&self.app, "GET", "/settings/ibkr_connect", None, Csrf::None, &[]).await;
        assert_eq!(answer.status, 200);
        answer.body
    }
    async fn post(&mut self, path: &str, form: &[(&str, &str)]) -> u16 {
        self.page().await;
        self.browser.send(&self.app, "POST", path, Some(form), Csrf::Form, &[]).await.status
    }
    /// The key's (status, signing, encryption, dh) as stored, decrypted.
    fn key(&self) -> Option<(i64, String, String, String)> {
        use rusqlite::OptionalExtension;
        let row: Option<(i64, String, String, String)> = self.c.query_row("SELECT status, rsa_signature_key, rsa_encryption_key, dh_param FROM api_keys WHERE user_id = ?1 AND exchange_id = ?2 AND key_type = 0",
                                                                         (self.user, self.exchange), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional().unwrap();
        row.map(|(s, a, b, d)| (s, self.app.cipher.decrypt(&a).unwrap(), self.app.cipher.decrypt(&b).unwrap(), self.app.cipher.decrypt(&d).unwrap()))
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_new_connection_makes_its_keys_inline_and_serves_their_public_halves() {
    let mut fx = hosted().await;
    assert!(fx.page().await.contains("Start setup"));
    assert_eq!(fx.post("/settings/ibkr_connect", &[]).await, 302);
    let (status, signing, encryption, dh) = fx.key().expect("the key is made");
    assert_eq!(status, 0);
    assert_eq!(dh, signer::ffdhe2048_pem(), "D-IBKR-DHGROUP");
    assert!(signer::usable(&signing, &encryption, &dh));
    assert_ne!(signing, encryption);
    let page = fx.page().await;
    assert!(page.contains("artifact=signing") && page.contains("api_key[access_token]"), "the downloads, the portal steps and the paste form");
    for (artifact, expected) in [("signing", signer::public_pem(&signing).unwrap()), ("encryption", signer::public_pem(&encryption).unwrap()), ("dhparam", dh.clone())] {
        let answer = fx.browser.send(&fx.app, "GET", &format!("/settings/ibkr_connect/download?artifact={artifact}"), None, Csrf::None, &[]).await;
        assert_eq!((answer.status, answer.body.as_str()), (200, expected.as_str()), "{artifact}");
        assert!(!answer.body.contains("PRIVATE"));
    }
    // Starting again keeps the keys the user may have uploaded already.
    fx.browser.send(&fx.app, "POST", "/settings/ibkr_connect", None, Csrf::Header, &[]).await;
    assert_eq!(fx.key().map(|k| k.1), Some(signing));
}

#[tokio::test(flavor = "current_thread")]
async fn a_key_whose_material_makes_no_session_offers_disconnect_only_and_is_never_completed() {
    let mut fx = hosted().await;
    let now = "2026-09-01 00:00:00";
    // A Rails key whose job never ran (no RSA keys), then one whose material this build cannot read.
    fx.c.execute("INSERT INTO api_keys(user_id, exchange_id, key_type, status, created_at, updated_at) VALUES(?1, ?2, 0, 3, ?3, ?3)", (fx.user, fx.exchange, now)).unwrap();
    for material in [None, Some("not a key")] {
        if let Some(text) = material {
            let e = fx.app.cipher.encrypt(text);
            fx.c.execute("UPDATE api_keys SET rsa_signature_key = ?1, rsa_encryption_key = ?1, dh_param = ?1", [e]).unwrap();
        }
        let page = fx.page().await;
        assert!(page.contains("be used now") && page.contains(">Disconnect</button>"), "{page}");
        assert!(!page.contains("api_key[access_token]") && !page.contains("Start over") && !page.contains("aria-busy"));
        let row = |c: &rusqlite::Connection| c.query_row("SELECT count(*), max(rsa_signature_key), max(dh_param), max(status) FROM api_keys", [], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, i64>(3)?))).unwrap();
        let before = row(&fx.c);
        assert_eq!(fx.post("/settings/ibkr_connect", &[]).await, 302);
        assert_eq!(row(&fx.c), before);
        assert!(fx.page().await.contains("be used now"), "a create converts nothing");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn disconnect_drops_the_key_and_its_binding_and_keeps_its_ledger_rows() {
    let mut fx = hosted().await;
    assert_eq!(fx.post("/settings/ibkr_connect", &[]).await, 302);
    let id: i64 = fx.c.query_row("SELECT id FROM api_keys", [], |r| r.get(0)).unwrap();
    deltabadger::venue::ibkr_account::bind(&fx.c, &fx.app.cipher, id, "U1234567", chrono::Utc::now()).unwrap().unwrap();
    fx.c.execute("INSERT INTO account_transactions(user_id, exchange_id, api_key_id, base_amount, base_currency, entry_type, transacted_at, created_at, updated_at) \
                  VALUES(?1, ?2, ?3, 1, 'AAPL', 0, '2026-09-01 00:00:00', '2026-09-01 00:00:00', '2026-09-01 00:00:00')", (fx.user, fx.exchange, id)).unwrap();
    assert_eq!(fx.post("/settings/ibkr_connect", &[("_method", "delete")]).await, 302);
    assert!(fx.key().is_none());
    assert!(deltabadger::venue::ibkr_account::binding(&fx.c, id).unwrap().is_none());
    assert_eq!(fx.c.query_row("SELECT count(*), max(api_key_id) FROM account_transactions", [], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))).unwrap(), (1, None));
    assert!(fx.page().await.contains("Start setup"));
}
