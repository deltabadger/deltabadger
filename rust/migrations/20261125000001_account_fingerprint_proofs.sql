-- #710 slice 7a rules N2, P3, N7: an account's fingerprint backs a credential fence only once a key read proved it
-- (`account_fingerprint_proofs`, written whenever a key read establishes or confirms the fingerprint; a file never
-- does), and an account's ledger reader generation (`ledger_reader_generations`, bumped whenever the key the account's
-- ledger and balances are read with changes, so a run of the former reader publishes nothing). Rust-only: 2.x never
-- reads either.
CREATE TABLE account_fingerprint_proofs (
  account_id INTEGER PRIMARY KEY,
  created_at TEXT NOT NULL
);
CREATE TABLE ledger_reader_generations (
  account_id INTEGER PRIMARY KEY,
  generation INTEGER NOT NULL,
  reader INTEGER
);
-- Rule P3: a fingerprint is proven when a key read it (a credential observation names it; the takeover's
-- `unconfirmed-after-2.x` sentinel names nothing), or when no import observation of the account names it (a file could
-- not have established it).
INSERT INTO account_fingerprint_proofs (account_id, created_at)
SELECT a.id, strftime('%Y-%m-%d %H:%M:%S', 'now') FROM exchange_accounts a
 WHERE a.fingerprint IS NOT NULL AND a.removed_at IS NULL
   AND (EXISTS (SELECT 1 FROM exchange_account_credentials o JOIN api_keys k ON k.id = o.api_key_id
                 WHERE k.user_id = a.user_id AND k.exchange_id = a.exchange_id AND o.fingerprint = a.fingerprint
                   AND o.fingerprint <> 'unconfirmed-after-2.x')
        OR NOT EXISTS (SELECT 1 FROM exchange_account_imports i WHERE i.account_id = a.id AND i.fingerprint = a.fingerprint));
