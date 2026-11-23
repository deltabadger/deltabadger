-- #710 slice 7a rule N8: which ledger rows a file import wrote (`import_row_origins`), recorded in the insert's own
-- transaction so no later enrichment or repair of the row's raw data changes it; a row's origin goes with the row.
-- `import_kind`: the importer (`deltabadger`, `binance`, `kucoin`), `binance_accepted` for a Binance export row the user
-- imported past a warning (its retained id stripped), `v2` for a row 2.x's importer wrote while it owned the install.
-- Rule N4: each answer to a parked question (an import's, a key save's) is consumed once (`parked_answers`, by the
-- answer's digest), in the write it allows. Rust-only: 2.x never reads either.
CREATE TABLE import_row_origins (
  transaction_id INTEGER PRIMARY KEY REFERENCES account_transactions (id) ON DELETE CASCADE,
  import_kind TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE parked_answers (
  digest TEXT PRIMARY KEY,
  created_at TEXT NOT NULL
);
