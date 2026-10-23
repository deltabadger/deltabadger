-- KuCoin account-history import (plan KI §5): each class's API takeover cutoff per (user, exchange), in a Rust table so
-- Rails' encryption reset (which deletes every app_configs row) never drops it. The cutoffs earlier builds kept in
-- app_configs (`rust_kucoin_csv.cutoff:<user>:<exchange>`, JSON {class: time}) move here.
CREATE TABLE import_cutoffs (
  id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL,
  exchange_id INTEGER NOT NULL,
  class TEXT NOT NULL,
  cutoff_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE (user_id, exchange_id, class)
);
INSERT INTO import_cutoffs (user_id, exchange_id, class, cutoff_at, updated_at)
  SELECT CAST(substr(rest, 1, instr(rest, ':') - 1) AS INTEGER), CAST(substr(rest, instr(rest, ':') + 1) AS INTEGER), j.key, j.value, cfg.updated_at
  FROM (SELECT substr(key, 24) AS rest, value, updated_at FROM app_configs
        WHERE substr(key, 1, 23) = 'rust_kucoin_csv.cutoff:' AND json_valid(value)) cfg, json_each(cfg.value) j
  WHERE j.key IN ('fills', 'deposits', 'withdrawals') AND j.type = 'text';
DELETE FROM app_configs WHERE substr(key, 1, 23) = 'rust_kucoin_csv.cutoff:';
