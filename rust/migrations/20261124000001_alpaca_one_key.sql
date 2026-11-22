-- D-ALPACA-ONE-KEY: Alpaca has one key and no permissions, so it is the account's trading key (key_type 0) whoever saved it.
-- A tracker key (read-only, 2) becomes it where its account has no trading key yet; an account holding both keeps both.
UPDATE api_keys SET key_type = 0
 WHERE key_type = 2
   AND exchange_id IN (SELECT id FROM exchanges WHERE type = 'Exchanges::Alpaca')
   AND NOT EXISTS (SELECT 1 FROM api_keys t WHERE t.user_id = api_keys.user_id AND t.exchange_id = api_keys.exchange_id AND t.key_type = 0
                   AND (SELECT account_id FROM exchange_account_keys WHERE api_key_id = t.id) IS (SELECT account_id FROM exchange_account_keys WHERE api_key_id = api_keys.id));
