//! #710 slice 4 (PLAN-710-4 §6): one user with two accounts on an exchange, inserted directly (a second account cannot be
//! made otherwise until slice 7), a key and a bot on each, and the venue ids both accounts share.
use super::seed::{self, BotSpec, Seeded};
use deltabadger::crypto::Cipher;
use deltabadger::identity::Account;
use deltabadger::venue_ids::{self, Scope};
use rusqlite::{params, Connection};

/// The ledger id, order id and group id both accounts hold.
pub const SHARED_TX: &str = "shared-tx";
pub const SHARED_ORDER: &str = "shared-order";
pub const SHARED_GROUP: &str = "g1";

/// One exchange of the user with accounts number 1 and 2: `seeded` names the first account's key (its `api_key_id`),
/// `keys`/`bots`/`accounts`/`scopes` are [number 1, number 2].
pub struct Two {
    pub seeded: Seeded,
    pub accounts: [Account; 2],
    pub scopes: [Scope; 2],
    pub keys: [i64; 2],
    pub bots: [i64; 2],
}

impl Two {
    /// The seeded install's Kraken (seed::seed_kraken) with a second account.
    pub fn kraken(c: &Connection, cipher: &Cipher) -> Self {
        let seeded = seed::seed_kraken(c, cipher);
        Self::on(c, cipher, seeded)
    }

    /// Another exchange of `of`'s user (`type_` as 2.x names it, e.g. `Exchanges::Binance`), with a trading key, and a second account.
    pub fn venue(c: &Connection, cipher: &Cipher, of: &Seeded, type_: &str, name: &str) -> Self {
        c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES (?1, ?2, '0.1', '0.1', 'now', 'now')", params![type_, name]).unwrap();
        let exchange_id = c.last_insert_rowid();
        c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
                   minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) \
                   VALUES (?1, 'BTCEUR', 'BTC', 'EUR', ?2, ?3, 8, 2, 2, '0.00001', '1', 1, 1, 'now', 'now')", params![exchange_id, of.btc, of.quote]).unwrap();
        let ticker_id = c.last_insert_rowid();
        c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 1, 0, 'now', 'now')",
                  params![of.user_id, exchange_id, cipher.encrypt("account-1-key"), cipher.encrypt("account-1-secret")]).unwrap();
        let seeded = Seeded { user_id: of.user_id, exchange_id, btc: of.btc, quote: of.quote, ticker_id, api_key_id: c.last_insert_rowid() };
        Self::on(c, cipher, seeded)
    }

    fn on(c: &Connection, cipher: &Cipher, seeded: Seeded) -> Self {
        let now = chrono::Utc::now();
        let first = Account::first(c, seeded.user_id, seeded.exchange_id).unwrap().ensure(c, now).unwrap();
        c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at) VALUES (?1, ?2, 2, 'now', 'now')",
                  [seeded.user_id, seeded.exchange_id]).unwrap();
        let second = Account::by_id(c, c.last_insert_rowid()).unwrap().unwrap();
        c.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 1, 0, 'now', 'now')",
                  params![seeded.user_id, seeded.exchange_id, cipher.encrypt("account-2-key"), cipher.encrypt("account-2-secret")]).unwrap();
        let second_key = c.last_insert_rowid();
        second.add_key(c, second_key).unwrap();
        let mut bots = [0; 2];
        for (n, account) in [first, second].into_iter().enumerate() {
            bots[n] = seed::insert_bot(c, &seeded, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
            deltabadger::identity::place_bot(c, bots[n], seeded.user_id, seeded.exchange_id, Some(account), now).unwrap();
        }
        let scopes = [Scope::of(c, &first).unwrap(), Scope::of(c, &second).unwrap()];
        Self { keys: [seeded.api_key_id, second_key], seeded, accounts: [first, second], scopes, bots }
    }

    /// Account number 3 of the exchange, with no key or bot.
    pub fn third(&self, c: &Connection) -> Account {
        c.execute("INSERT INTO exchange_accounts (user_id, exchange_id, number, created_at, updated_at) VALUES (?1, ?2, 3, 'now', 'now')",
                  [self.seeded.user_id, self.seeded.exchange_id]).unwrap();
        Account::by_id(c, c.last_insert_rowid()).unwrap().unwrap()
    }

    /// A withdrawal rule of account `n` on `asset` with `status` (engine::rules::create, then its status, mirrored for number 1).
    pub fn rule(&self, c: &Connection, cipher: &Cipher, n: usize, asset: i64, status: i64) -> i64 {
        let id = deltabadger::engine::rules::create(c, &self.accounts[n], asset, &cipher.encrypt("bc1-address"), "{}", "2026-09-01 10:00:00").unwrap();
        c.execute("UPDATE exchange_account_rules SET status = ?2 WHERE id = ?1", [id, status]).unwrap();
        deltabadger::engine::rules::mirror(c, Some(id)).unwrap();
        id
    }

    /// A bot order of account `n`'s bot with venue id `external`, `status` and `external_status`, written as placement writes
    /// one: `stored` into 2.x's column, then `after_order_insert`. Returns its id.
    pub fn order(&self, c: &Connection, n: usize, external: &str, status: i64, external_status: Option<i64>) -> i64 {
        let scope = &self.scopes[n];
        let id = seed::insert_tx(c, &self.seeded, self.bots[n], &seed::TxSpec { status, external_status, external_id: scope.stored(Some(external)).map(str::to_string), order_type: 0,
            amount: Some("0.001"), quote_amount: Some("60"), price: Some("60000"), quote_amount_exec: None, amount_exec: None, created_at: "2026-09-02 10:00:00".into() });
        venue_ids::after_order_insert(c, scope, id, Some(external)).unwrap();
        id
    }

    /// Account `n`'s fingerprint (`identity::establish` fills only an unknown one; a fixture writes any).
    pub fn fingerprint(&self, c: &Connection, n: usize, fingerprint: Option<&str>) {
        c.execute("UPDATE exchange_accounts SET fingerprint = ?2 WHERE id = ?1", params![self.id(n), fingerprint]).unwrap();
    }

    /// A trade row of account `n` (`entry_type` 0-3) with venue id `id` and its facts: `base_amount` of BTC for 100 EUR at `at`.
    pub fn trade(&self, c: &Connection, n: usize, entry_type: i64, id: &str, base_amount: &str, at: &str) -> i64 {
        let row = self.row(c, n, entry_type, "BTC", base_amount, Some(id), None, at);
        c.execute("UPDATE account_transactions SET quote_currency = 'EUR', quote_amount = '100' WHERE id = ?1", [row]).unwrap();
        row
    }

    /// A recorded venue withdrawal id of account `n` (`rust_withdrawal_ids`), owned by `owner`.
    pub fn withdrawal_id(&self, c: &Connection, n: usize, id: &str, owner: &str) {
        c.execute("INSERT INTO rust_withdrawal_ids (account_id, exchange_id, venue_id, owner, settled_at, source) VALUES (?1, ?2, ?3, ?4, 'now', 'operator')",
                  params![self.id(n), self.seeded.exchange_id, id, owner]).unwrap();
    }

    /// A standalone API order of account `n` in `state` with `outcome` (JSON text or None), placed as venue id `external`.
    pub fn standalone(&self, c: &Connection, n: usize, state: &str, outcome: Option<&str>, external: Option<&str>) -> i64 {
        c.execute("INSERT INTO rust_api_orders (user_id, bot_id, side, amount, amount_type, supplied, state, outcome, created_at, send_by) \
                   VALUES (?1, NULL, 'buy', '10', 'quote', '{}', ?2, ?3, '2026-09-20 10:00:00', '2026-09-20 10:01:00')", params![self.seeded.user_id, state, outcome]).unwrap();
        let id = c.last_insert_rowid();
        c.execute("INSERT INTO rust_api_order_standalone (order_id, account_id, exchange_id, ticker_id, order_type, surface, external_id, order_snapshot) \
                   VALUES (?1, ?2, ?3, ?4, 'limit', 'rest', ?5, '{\"symbol\":\"BTCEUR\",\"side\":\"buy\",\"limit\":true}')",
                  params![id, self.id(n), self.seeded.exchange_id, self.seeded.ticker_id, external]).unwrap();
        id
    }

    /// Account `n`'s (0: number 1, 1: number 2) row id.
    pub fn id(&self, n: usize) -> i64 { self.accounts[n].id.unwrap() }

    /// A ledger row of account `n` (0: number 1, 1: number 2) written as every writer writes one: `stored` into 2.x's
    /// column, then `after_insert`. Returns its id.
    #[allow(clippy::too_many_arguments)]
    pub fn row(&self, c: &Connection, n: usize, entry_type: i64, currency: &str, amount: &str, id: Option<&str>, group: Option<&str>, at: &str) -> i64 {
        let scope = &self.scopes[n];
        c.execute("INSERT INTO account_transactions (user_id, api_key_id, exchange_id, entry_type, base_currency, base_amount, tx_id, group_id, transacted_at, \
                   raw_data, manual_values, transfer_link_rejected, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, '{}', '{}', 0, ?9, ?9)",
                  params![self.seeded.user_id, self.keys[n], self.seeded.exchange_id, entry_type, currency, amount, scope.stored(id), group, at]).unwrap();
        let row = c.last_insert_rowid();
        venue_ids::after_insert(c, scope, row, id).unwrap();
        row
    }
}
