-- Rails' answer, frozen from the commit tagged rails-oracle-final (PLAN-B11 B11-1).
PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
CREATE TABLE "action_mcp_sessions" ("id" varchar NOT NULL PRIMARY KEY, "client_capabilities" json, "client_info" json, "consents" json DEFAULT '{}' NOT NULL, "created_at" datetime(6) NOT NULL, "ended_at" datetime(6), "initialized" boolean DEFAULT FALSE NOT NULL, "messages_count" integer DEFAULT 0 NOT NULL, "prompt_registry" json DEFAULT '[]', "protocol_version" varchar, "resource_registry" json DEFAULT '[]', "role" varchar DEFAULT 'server' NOT NULL, "server_capabilities" json, "server_info" json, "session_data" json DEFAULT '{}' NOT NULL, "status" varchar DEFAULT 'pre_initialize' NOT NULL, "tool_registry" json DEFAULT '[]', "updated_at" datetime(6) NOT NULL);
CREATE TABLE "app_configs" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "created_at" datetime(6) NOT NULL, "key" varchar NOT NULL, "updated_at" datetime(6) NOT NULL, "value" text);
INSERT INTO "app_configs" (rowid, "id", "created_at", "key", "updated_at", "value") VALUES (1, 1, '2026-11-16 07:39:06.003115', 'market_data_provider', '2026-11-16 07:39:06.003115', '{"p":"jMLYYwbelAshsfQ=","h":{"iv":"aSFkTPCJDClbOtnp","at":"N4T1AE+fF5CP1dr8N/Hy1Q=="}}');
INSERT INTO "app_configs" (rowid, "id", "created_at", "key", "updated_at", "value") VALUES (2, 2, '2026-11-16 07:39:06.003969', 'market_data_url', '2026-11-16 07:39:06.003969', '{"p":"grjlOru/TGVTJ+qJs2VTNYGgNBM=","h":{"iv":"2O480SemNsfY3VZg","at":"FIfPWF7iWsR0tO6aCMF9iA=="}}');
INSERT INTO "app_configs" (rowid, "id", "created_at", "key", "updated_at", "value") VALUES (3, 3, '2026-11-16 07:39:06.004517', 'market_data_token', '2026-11-16 07:39:06.004517', '{"p":"C8WKlcM=","h":{"iv":"gmUZ3dW0BHWQXBC4","at":"O93jM3lHDcUS7poUMDPlmA=="}}');
CREATE TABLE "assets" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "category" varchar, "circulating_supply" decimal(30,8), "color" varchar, "country" varchar, "country_exchange" varchar, "created_at" datetime(6) NOT NULL, "external_id" varchar NOT NULL, "image_url" varchar, "instrument_type" varchar, "isin" varchar, "market_cap" bigint, "market_cap_rank" integer, "name" varchar, "symbol" varchar, "updated_at" datetime(6) NOT NULL, "url" varchar);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (1, 1, 'Fiat', NULL, NULL, NULL, NULL, '2026-11-16 07:39:01.500145', 'usd', NULL, NULL, NULL, NULL, NULL, 'US Dollar', 'USD', '2026-11-16 07:39:01.500145', NULL);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (2, 2, 'Stock', NULL, NULL, NULL, NULL, '2026-11-16 07:39:01.500877', 'stock-aaa', NULL, 'stock', NULL, NULL, NULL, 'AAA Inc.', 'AAA', '2026-11-16 07:39:01.500877', NULL);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (3, 3, 'Stock', NULL, NULL, NULL, NULL, '2026-11-16 07:39:01.501388', 'stock-bbb', NULL, 'etf', NULL, NULL, NULL, 'BBB Inc.', 'BBB', '2026-11-16 07:39:01.501388', NULL);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (4, 4, 'Stock', NULL, NULL, NULL, NULL, '2026-11-16 07:39:01.501856', 'stock-ccc', NULL, 'stock', NULL, NULL, NULL, 'CCC Inc.', 'CCC', '2026-11-16 07:39:01.501856', NULL);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (5, 5, 'Stock', NULL, NULL, NULL, NULL, '2026-11-16 07:39:01.502318', 'stock-ddd', NULL, 'etf', NULL, NULL, NULL, 'DDD Inc.', 'DDD', '2026-11-16 07:39:01.502318', NULL);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (6, 6, 'Stock', NULL, NULL, NULL, NULL, '2026-11-16 07:39:01.502775', 'stock-eee', NULL, 'stock', NULL, NULL, NULL, 'EEE Inc.', 'EEE', '2026-11-16 07:39:01.502775', NULL);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (7, 7, 'Stock', NULL, NULL, NULL, NULL, '2026-11-16 07:39:01.503235', 'stock-fff', NULL, 'etf', NULL, NULL, NULL, 'FFF Inc.', 'FFF', '2026-11-16 07:39:01.503235', NULL);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (8, 8, 'Stock', NULL, NULL, NULL, NULL, '2026-11-16 07:39:01.503694', 'stock-ggg', NULL, 'stock', NULL, NULL, NULL, 'GGG Inc.', 'GGG', '2026-11-16 07:39:01.503694', NULL);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (9, 9, 'Stock', NULL, NULL, NULL, NULL, '2026-11-16 07:39:01.504147', 'stock-hhh', NULL, 'etf', NULL, NULL, NULL, 'HHH Inc.', 'HHH', '2026-11-16 07:39:01.504147', NULL);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (10, 10, 'Stock', NULL, NULL, NULL, NULL, '2026-11-16 07:39:01.504737', 'stock-iii', NULL, 'stock', NULL, NULL, NULL, 'III Inc.', 'III', '2026-11-16 07:39:01.504737', NULL);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (11, 11, 'Stock', NULL, NULL, NULL, NULL, '2026-11-16 07:39:01.505234', 'stock-jjj', NULL, 'etf', NULL, NULL, NULL, 'JJJ Inc.', 'JJJ', '2026-11-16 07:39:01.505234', NULL);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (12, 12, 'Stock', NULL, NULL, NULL, NULL, '2026-11-16 07:39:01.505702', 'stock-kkk', NULL, 'stock', NULL, NULL, NULL, 'KKK Inc.', 'KKK', '2026-11-16 07:39:01.505702', NULL);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (13, 13, 'Stock', NULL, NULL, NULL, NULL, '2026-11-16 07:39:01.506162', 'stock-lll', NULL, 'etf', NULL, NULL, NULL, 'LLL Inc.', 'LLL', '2026-11-16 07:39:01.506162', NULL);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (14, 14, 'Cryptocurrency', NULL, NULL, NULL, NULL, '2026-11-16 07:39:01.506611', 'bitcoin', NULL, NULL, NULL, NULL, NULL, 'Bitcoin', 'BTC', '2026-11-16 07:39:01.506611', NULL);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (15, 15, 'Cryptocurrency', NULL, NULL, NULL, NULL, '2026-11-16 07:39:01.507052', 'ethereum', NULL, NULL, NULL, NULL, NULL, 'Ethereum', 'ETH', '2026-11-16 07:39:01.507052', NULL);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (16, 16, 'Cryptocurrency', NULL, NULL, NULL, NULL, '2026-11-16 07:39:01.507482', 'tether', NULL, NULL, NULL, NULL, NULL, 'Tether', 'USDT', '2026-11-16 07:39:01.507482', NULL);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (17, 17, 'Stock', NULL, NULL, NULL, NULL, '2026-11-16 07:39:01.507919', 'stock-aaa-other', NULL, NULL, NULL, NULL, NULL, 'Another AAA', 'AAA', '2026-11-16 07:39:01.507919', NULL);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (18, 18, 'Stock', NULL, NULL, NULL, NULL, '2026-11-16 07:39:01.508365', 'stock-named', NULL, NULL, NULL, NULL, NULL, 'Nameco', NULL, '2026-11-16 07:39:01.508365', NULL);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (19, 19, 'Stock', NULL, NULL, NULL, NULL, '2026-11-16 07:39:01.508787', 'stock-bare', NULL, NULL, NULL, NULL, NULL, '', '', '2026-11-16 07:39:01.508787', NULL);
CREATE TABLE "exchanges" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "available" boolean DEFAULT TRUE, "created_at" datetime NOT NULL, "maker_fee" varchar, "name" varchar, "taker_fee" varchar, "type" varchar, "updated_at" datetime NOT NULL);
INSERT INTO "exchanges" (rowid, "id", "available", "created_at", "maker_fee", "name", "taker_fee", "type", "updated_at") VALUES (1, 1, 1, '2026-11-16 07:39:01.469396', '0.15', 'Alpaca', '0.25', 'Exchanges::Alpaca', '2026-11-16 07:39:01.469396');
CREATE TABLE "fx_rates" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "currency" varchar NOT NULL, "date" date NOT NULL, "rate" decimal NOT NULL);
CREATE TABLE "historical_prices" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "asset" varchar NOT NULL, "currency" varchar NOT NULL, "date" date NOT NULL, "price" decimal NOT NULL);
CREATE TABLE "ibkr_locks" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "created_at" datetime(6) NOT NULL, "expires_at" datetime(6) NOT NULL, "key" varchar NOT NULL, "owner" varchar NOT NULL, "updated_at" datetime(6) NOT NULL);
CREATE TABLE "indices" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "available_exchanges" json DEFAULT '{}', "created_at" datetime(6) NOT NULL, "description" text, "external_id" varchar, "market_cap" decimal, "name" varchar, "source" varchar, "top_coins" json, "top_coins_by_exchange" json DEFAULT '{}', "updated_at" datetime(6) NOT NULL, "weight" integer DEFAULT 0 NOT NULL, "weights" json DEFAULT '{}');
CREATE TABLE "oauth_access_grants" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "application_id" integer NOT NULL, "code_challenge" varchar, "code_challenge_method" varchar, "created_at" datetime(6) NOT NULL, "expires_in" integer NOT NULL, "redirect_uri" text NOT NULL, "resource_owner_id" integer NOT NULL, "revoked_at" datetime(6), "scopes" varchar DEFAULT '' NOT NULL, "token" varchar NOT NULL);
CREATE TABLE "oauth_access_tokens" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "application_id" integer NOT NULL, "created_at" datetime(6) NOT NULL, "expires_in" integer, "previous_refresh_token" varchar DEFAULT '' NOT NULL, "refresh_token" varchar, "resource_owner_id" integer, "revoked_at" datetime(6), "scopes" varchar DEFAULT '' NOT NULL, "token" varchar NOT NULL);
CREATE TABLE "portfolio_snapshots" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "created_at" datetime(6) NOT NULL, "date" date NOT NULL, "held_cost_usd" decimal(20,8), "held_value_usd" decimal(20,8), "invested_usd" decimal(20,8) DEFAULT 0.0 NOT NULL, "partial" boolean DEFAULT FALSE NOT NULL, "updated_at" datetime(6) NOT NULL, "user_id" integer NOT NULL, "value_usd" decimal(20,8) DEFAULT 0.0 NOT NULL);
CREATE TABLE "portfolio_venue_snapshots" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "user_id" integer NOT NULL, "exchange_id" integer NOT NULL, "date" date NOT NULL, "value_usd" decimal(20,8) DEFAULT 0.0 NOT NULL, "invested_usd" decimal(20,8) DEFAULT 0.0 NOT NULL, "held_value_usd" decimal(20,8), "held_cost_usd" decimal(20,8), "partial" boolean DEFAULT FALSE NOT NULL, "created_at" datetime(6) NOT NULL, "updated_at" datetime(6) NOT NULL);
CREATE TABLE "setting_flags" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "name" varchar, "value" boolean);
CREATE TABLE "users" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "admin" boolean DEFAULT FALSE NOT NULL, "advanced_bots_enabled" boolean DEFAULT FALSE NOT NULL, "confirmation_sent_at" datetime, "confirmation_token" varchar, "confirmed_at" datetime, "created_at" datetime NOT NULL, "display_currency" varchar DEFAULT 'USD' NOT NULL, "email" varchar DEFAULT '' NOT NULL, "encrypted_password" varchar DEFAULT '' NOT NULL, "failed_attempts" integer DEFAULT 0 NOT NULL, "hide_balances" boolean DEFAULT FALSE NOT NULL, "last_otp_at" datetime, "locale" varchar, "locked_at" datetime(6), "mcp_settings" json DEFAULT '{}', "name" varchar, "otp_module" integer DEFAULT 0, "otp_secret_key" varchar, "remember_created_at" datetime, "reset_password_sent_at" datetime, "reset_password_token" varchar, "rest_settings" json DEFAULT '{}', "setup_completed" boolean DEFAULT FALSE NOT NULL, "show_smart_intervals_info" boolean DEFAULT TRUE NOT NULL, "subscribed_to_email_marketing" boolean DEFAULT TRUE, "time_zone" varchar DEFAULT 'UTC' NOT NULL, "tracker_settings" json DEFAULT '{}', "unconfirmed_email" varchar, "updated_at" datetime NOT NULL, "wash_sale_enabled" boolean, "wash_sale_jurisdiction" varchar, "wash_sale_selling_warned_at" datetime(6));
INSERT INTO "users" (rowid, "id", "admin", "advanced_bots_enabled", "confirmation_sent_at", "confirmation_token", "confirmed_at", "created_at", "display_currency", "email", "encrypted_password", "failed_attempts", "hide_balances", "last_otp_at", "locale", "locked_at", "mcp_settings", "name", "otp_module", "otp_secret_key", "remember_created_at", "reset_password_sent_at", "reset_password_token", "rest_settings", "setup_completed", "show_smart_intervals_info", "subscribed_to_email_marketing", "time_zone", "tracker_settings", "unconfirmed_email", "updated_at", "wash_sale_enabled", "wash_sale_jurisdiction", "wash_sale_selling_warned_at") VALUES (1, 1, 1, 0, NULL, NULL, '2026-11-16 07:39:01.263542', '2026-11-16 07:39:01.432071', 'EUR', 'owner@example.com', '$2a$11$ecInj0hwYaiRSPRnz3zfEeqsaJvb4/M6BEHjZzs0qhm3hh8pEeWQK', 0, 0, NULL, NULL, NULL, '{}', 'Owner', 0, '{"p":"0iBrMP51DiVxIRTZEuQ4rlUbrWZAsULKq+xj9gh4b+c=","h":{"iv":"e+qslpg2RrvmbJz7","at":"NsLKppGNLC2s0OXOY/UqUg==","e":"QVNDSUktOEJJVA=="}}', NULL, NULL, NULL, '{}', 1, 1, 1, 'Warsaw', '{}', NULL, '2026-11-16 07:39:01.432071', NULL, NULL, NULL);
CREATE TABLE "account_balances" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "asset_id" integer NOT NULL, "created_at" datetime(6) NOT NULL, "exchange_id" integer NOT NULL, "free" decimal(32,16) DEFAULT 0.0 NOT NULL, "locked" decimal(32,16) DEFAULT 0.0 NOT NULL, "priced_at" datetime(6), "synced_at" datetime(6) NOT NULL, "updated_at" datetime(6) NOT NULL, "usd_price" decimal(20,8), "usd_value" decimal(20,8), "user_id" integer NOT NULL, CONSTRAINT "fk_rails_4c03793396"
FOREIGN KEY ("exchange_id")
  REFERENCES "exchanges" ("id")
, CONSTRAINT "fk_rails_c4876d5668"
FOREIGN KEY ("asset_id")
  REFERENCES "assets" ("id")
, CONSTRAINT "fk_rails_30b52bf707"
FOREIGN KEY ("user_id")
  REFERENCES "users" ("id")
);
CREATE TABLE "account_transactions" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "api_key_id" integer, "base_amount" decimal NOT NULL, "base_asset_id" integer, "base_currency" varchar NOT NULL, "created_at" datetime(6) NOT NULL, "description" varchar, "entry_type" integer NOT NULL, "exchange_id" integer NOT NULL, "fee_amount" decimal, "fee_currency" varchar, "group_id" varchar, "linked_transaction_id" integer, "manual_values" json DEFAULT '{}', "quote_amount" decimal, "quote_currency" varchar, "raw_data" json DEFAULT '{}', "transacted_at" datetime(6) NOT NULL, "transaction_id" integer, "transfer_link_rejected" boolean DEFAULT FALSE NOT NULL, "tx_id" varchar, "updated_at" datetime(6) NOT NULL, "user_id" integer NOT NULL, CONSTRAINT "fk_rails_51e23281dc"
FOREIGN KEY ("transaction_id")
  REFERENCES "transactions" ("id")
, CONSTRAINT "fk_rails_6cc5ad0293"
FOREIGN KEY ("api_key_id")
  REFERENCES "api_keys" ("id")
, CONSTRAINT "fk_rails_e38912f087"
FOREIGN KEY ("exchange_id")
  REFERENCES "exchanges" ("id")
, CONSTRAINT "fk_rails_5ab9b90923"
FOREIGN KEY ("user_id")
  REFERENCES "users" ("id")
);
CREATE TABLE "action_mcp_session_messages" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "created_at" datetime(6) NOT NULL, "direction" varchar DEFAULT 'client' NOT NULL, "is_ping" boolean DEFAULT FALSE NOT NULL, "jsonrpc_id" varchar, "message_json" json, "message_type" varchar NOT NULL, "request_acknowledged" boolean DEFAULT FALSE NOT NULL, "request_cancelled" boolean DEFAULT FALSE NOT NULL, "session_id" varchar NOT NULL, "updated_at" datetime(6) NOT NULL, CONSTRAINT "fk_rails_8d204792c3"
FOREIGN KEY ("session_id")
  REFERENCES "action_mcp_sessions" ("id")
 ON DELETE CASCADE ON UPDATE CASCADE);
CREATE TABLE "action_mcp_session_subscriptions" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "created_at" datetime(6) NOT NULL, "last_notification_at" datetime(6), "session_id" varchar NOT NULL, "updated_at" datetime(6) NOT NULL, "uri" varchar NOT NULL, CONSTRAINT "fk_rails_a643941a8d"
FOREIGN KEY ("session_id")
  REFERENCES "action_mcp_sessions" ("id")
 ON DELETE CASCADE);
CREATE TABLE "action_mcp_session_tasks" ("id" varchar NOT NULL PRIMARY KEY, "continuation_state" json DEFAULT '{}', "created_at" datetime(6) NOT NULL, "last_step_at" datetime(6), "last_updated_at" datetime(6) NOT NULL, "poll_interval" integer, "progress_message" varchar, "progress_percent" integer, "request_method" varchar, "request_name" varchar, "request_params" json, "result_payload" json, "session_id" varchar NOT NULL, "status" varchar DEFAULT 'working' NOT NULL, "status_message" varchar, "ttl" integer, "updated_at" datetime(6) NOT NULL, CONSTRAINT "fk_rails_a7fc0e6f31"
FOREIGN KEY ("session_id")
  REFERENCES "action_mcp_sessions" ("id")
 ON DELETE CASCADE ON UPDATE CASCADE);
CREATE TABLE "api_keys" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "access_token" text, "balances_synced_at" datetime(6), "created_at" datetime NOT NULL, "dh_param" text, "exchange_id" bigint NOT NULL, "german_trading_agreement" boolean, "ibkr_realm" varchar, "key" varchar, "key_type" integer DEFAULT 0 NOT NULL, "last_sync_error" varchar, "last_synced_at" datetime(6), "passphrase" varchar, "rsa_encryption_key" text, "rsa_signature_key" text, "secret" varchar, "status" integer DEFAULT 0 NOT NULL, "updated_at" datetime NOT NULL, "user_id" bigint NOT NULL, CONSTRAINT "fk_rails_7601a65574"
FOREIGN KEY ("exchange_id")
  REFERENCES "exchanges" ("id")
, CONSTRAINT "fk_rails_32c28d0dc2"
FOREIGN KEY ("user_id")
  REFERENCES "users" ("id")
);
INSERT INTO "api_keys" (rowid, "id", "access_token", "balances_synced_at", "created_at", "dh_param", "exchange_id", "german_trading_agreement", "ibkr_realm", "key", "key_type", "last_sync_error", "last_synced_at", "passphrase", "rsa_encryption_key", "rsa_signature_key", "secret", "status", "updated_at", "user_id") VALUES (1, 1, NULL, NULL, '2026-11-16 07:39:01.489572', NULL, 1, NULL, NULL, '{"p":"jYQVfjDDEEsC3YJSaQihjES+2t1ZINsg5qcWbF6R","h":{"iv":"ChDBcvlPSNKgkm01","at":"zNOgM8VT0yvbxFX86P9BRA=="}}', 0, NULL, NULL, '{"p":"VjxZnIQ=","h":{"iv":"5lAD5QZKnb/aPUNG","at":"Z854C0dHDTXflQRFJKnnDA=="}}', NULL, NULL, '{"p":"A2NS7FGCGBDypPtWpNYLVfjhJXhlZpS78G7uheOSER9/","h":{"iv":"7hbT9DRphAL+ils0","at":"uws0d5eaktNIY6PexgyNWw=="}}', 1, '2026-11-16 07:39:01.489572', 1);
CREATE TABLE "bot_activity_logs" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "bot_id" integer NOT NULL, "created_at" datetime(6) NOT NULL, "details" json DEFAULT '{}' NOT NULL, "event" varchar NOT NULL, "level" integer DEFAULT 0 NOT NULL, "message" varchar, CONSTRAINT "fk_rails_88c7e92778"
FOREIGN KEY ("bot_id")
  REFERENCES "bots" ("id")
);
CREATE TABLE "bot_index_assets" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "asset_id" integer NOT NULL, "bot_id" integer NOT NULL, "created_at" datetime(6) NOT NULL, "current_allocation" decimal(10,6), "entered_at" datetime(6), "exited_at" datetime(6), "in_index" boolean DEFAULT TRUE, "target_allocation" decimal(10,6), "ticker_id" integer NOT NULL, "updated_at" datetime(6) NOT NULL, CONSTRAINT "fk_rails_57052ee8e9"
FOREIGN KEY ("bot_id")
  REFERENCES "bots" ("id")
, CONSTRAINT "fk_rails_cd91323d9c"
FOREIGN KEY ("asset_id")
  REFERENCES "assets" ("id")
, CONSTRAINT "fk_rails_cf136c56e9"
FOREIGN KEY ("ticker_id")
  REFERENCES "tickers" ("id")
);
INSERT INTO "bot_index_assets" (rowid, "id", "asset_id", "bot_id", "created_at", "current_allocation", "entered_at", "exited_at", "in_index", "target_allocation", "ticker_id", "updated_at") VALUES (1, 1, 2, 1, '2026-11-16 07:39:06.013926', NULL, '2026-11-16 07:39:06.013039', NULL, 1, 0.5, 1, '2026-11-16 07:39:06.013926');
INSERT INTO "bot_index_assets" (rowid, "id", "asset_id", "bot_id", "created_at", "current_allocation", "entered_at", "exited_at", "in_index", "target_allocation", "ticker_id", "updated_at") VALUES (2, 2, 3, 1, '2026-11-16 07:39:06.015138', NULL, '2026-11-16 07:39:06.014458', NULL, 1, 0.5, 2, '2026-11-16 07:39:06.015138');
INSERT INTO "bot_index_assets" (rowid, "id", "asset_id", "bot_id", "created_at", "current_allocation", "entered_at", "exited_at", "in_index", "target_allocation", "ticker_id", "updated_at") VALUES (3, 3, 14, 2, '2026-11-16 07:39:06.025306', NULL, '2026-11-16 07:39:06.024253', NULL, 1, 0.5, 18, '2026-11-16 07:39:06.025306');
INSERT INTO "bot_index_assets" (rowid, "id", "asset_id", "bot_id", "created_at", "current_allocation", "entered_at", "exited_at", "in_index", "target_allocation", "ticker_id", "updated_at") VALUES (4, 4, 15, 2, '2026-11-16 07:39:06.026529', NULL, '2026-11-16 07:39:06.025860', NULL, 1, 0.5, 19, '2026-11-16 07:39:06.026529');
INSERT INTO "bot_index_assets" (rowid, "id", "asset_id", "bot_id", "created_at", "current_allocation", "entered_at", "exited_at", "in_index", "target_allocation", "ticker_id", "updated_at") VALUES (5, 5, 4, 3, '2026-11-16 07:39:06.035257', NULL, '2026-11-16 07:39:06.034468', NULL, 1, 1, 3, '2026-11-16 07:39:06.035257');
INSERT INTO "bot_index_assets" (rowid, "id", "asset_id", "bot_id", "created_at", "current_allocation", "entered_at", "exited_at", "in_index", "target_allocation", "ticker_id", "updated_at") VALUES (6, 6, 5, 4, '2026-11-16 07:39:06.043128', NULL, '2026-11-16 07:39:06.042301', NULL, 1, 1, 4, '2026-11-16 07:39:06.043128');
INSERT INTO "bot_index_assets" (rowid, "id", "asset_id", "bot_id", "created_at", "current_allocation", "entered_at", "exited_at", "in_index", "target_allocation", "ticker_id", "updated_at") VALUES (7, 7, 3, 5, '2026-11-16 07:39:06.060287', NULL, '2026-11-16 07:39:06.059416', NULL, 1, 1, 2, '2026-11-16 07:39:06.060287');
CREATE TABLE "bot_signals" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "amount" decimal NOT NULL, "amount_type" integer DEFAULT 0 NOT NULL, "bot_id" integer NOT NULL, "created_at" datetime(6) NOT NULL, "direction" integer DEFAULT 0 NOT NULL, "enabled" boolean DEFAULT TRUE NOT NULL, "last_triggered_at" datetime(6), "token" varchar NOT NULL, "updated_at" datetime(6) NOT NULL, CONSTRAINT "fk_rails_37f3232e3f"
FOREIGN KEY ("bot_id")
  REFERENCES "bots" ("id")
);
CREATE TABLE "bots" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "account_balance" decimal DEFAULT 0.0, "created_at" datetime NOT NULL, "current_delay" integer DEFAULT 0 NOT NULL, "delay" integer DEFAULT 0 NOT NULL, "exchange_id" bigint, "fetch_restarts" integer DEFAULT 0 NOT NULL, "label" varchar, "last_end_of_funds_notification" datetime, "position" integer DEFAULT 0 NOT NULL, "redeploy_declined_offset" decimal DEFAULT 0.0 NOT NULL, "restarts" integer DEFAULT 0 NOT NULL, "restatement_generation" integer DEFAULT 0 NOT NULL, "settings" json DEFAULT '{}' NOT NULL, "settings_changed_at" datetime, "started_at" datetime, "status" integer DEFAULT 0 NOT NULL, "stop_message_key" varchar, "stopped_at" datetime, "transient_data" json DEFAULT '{}' NOT NULL, "type" varchar, "updated_at" datetime NOT NULL, "user_id" bigint, CONSTRAINT "fk_rails_331fb6c3bc"
FOREIGN KEY ("exchange_id")
  REFERENCES "exchanges" ("id")
, CONSTRAINT "fk_rails_16b7e3780a"
FOREIGN KEY ("user_id")
  REFERENCES "users" ("id")
);
INSERT INTO "bots" (rowid, "id", "account_balance", "created_at", "current_delay", "delay", "exchange_id", "fetch_restarts", "label", "last_end_of_funds_notification", "position", "redeploy_declined_offset", "restarts", "restatement_generation", "settings", "settings_changed_at", "started_at", "status", "stop_message_key", "stopped_at", "transient_data", "type", "updated_at", "user_id") VALUES (1, 1, 0, '2026-11-16 07:39:06.010737', 0, 0, 1, 0, 'usd', NULL, 1, 0, 0, 0, '{"quote_asset_id":1,"quote_amount":100.0,"interval":"week","weighting":"manual","allocations":{"2":0.5,"3":0.5},"smart_intervaled":false,"smart_interval_quote_amount":10.0,"limit_ordered":false,"limit_order_pcnt_distance":0.001,"quote_amount_limited":false,"quote_amount_limit":1000,"price_limited":false,"price_limit":1000000,"price_limit_range_lower_bound":0,"price_limit_range_upper_bound":1000000,"price_limit_timing_condition":"while","price_limit_value_condition":"below","price_limit_in_ticker_id":1,"price_drop_limited":false,"price_drop_limit":0.2,"price_drop_limit_time_window_condition":"ath","price_drop_limit_in_ticker_id":1,"moving_average_limited":false,"moving_average_limit_timing_condition":"while","moving_average_limit_value_condition":"below","moving_average_limit_in_ticker_id":1,"moving_average_limit_in_ma_type":"sma","moving_average_limit_in_timeframe":"one_day","moving_average_limit_in_period":9,"indicator_limited":false,"indicator_limit":30,"indicator_limit_timing_condition":"while","indicator_limit_value_condition":"below","indicator_limit_in_ticker_id":1,"indicator_limit_in_indicator":"rsi","indicator_limit_in_timeframe":"one_day","sell_price_limit_in_ticker_id":1,"sell_price_drop_limit_in_ticker_id":1,"sell_moving_average_limit_in_ticker_id":1,"sell_indicator_limit_in_ticker_id":1}', '2026-11-16 07:39:06.010679', '2026-03-02 14:30:00', 1, NULL, NULL, '{"missed_quote_amount":0,"missed_quote_amount_was_set":null}', 'Bots::DcaMultiAsset', '2026-11-16 07:39:06.010737', 1);
INSERT INTO "bots" (rowid, "id", "account_balance", "created_at", "current_delay", "delay", "exchange_id", "fetch_restarts", "label", "last_end_of_funds_notification", "position", "redeploy_declined_offset", "restarts", "restatement_generation", "settings", "settings_changed_at", "started_at", "status", "stop_message_key", "stopped_at", "transient_data", "type", "updated_at", "user_id") VALUES (2, 2, 0, '2026-11-16 07:39:06.022474', 0, 0, 1, 0, 'usdt', NULL, 2, 0, 0, 0, '{"quote_asset_id":16,"quote_amount":100.0,"interval":"week","weighting":"manual","allocations":{"14":0.5,"15":0.5},"smart_intervaled":false,"smart_interval_quote_amount":10.0,"limit_ordered":false,"limit_order_pcnt_distance":0.001,"quote_amount_limited":false,"quote_amount_limit":1000,"price_limited":false,"price_limit":1000000,"price_limit_range_lower_bound":0,"price_limit_range_upper_bound":1000000,"price_limit_timing_condition":"while","price_limit_value_condition":"below","price_limit_in_ticker_id":18,"price_drop_limited":false,"price_drop_limit":0.2,"price_drop_limit_time_window_condition":"ath","price_drop_limit_in_ticker_id":18,"moving_average_limited":false,"moving_average_limit_timing_condition":"while","moving_average_limit_value_condition":"below","moving_average_limit_in_ticker_id":18,"moving_average_limit_in_ma_type":"sma","moving_average_limit_in_timeframe":"one_day","moving_average_limit_in_period":9,"indicator_limited":false,"indicator_limit":30,"indicator_limit_timing_condition":"while","indicator_limit_value_condition":"below","indicator_limit_in_ticker_id":18,"indicator_limit_in_indicator":"rsi","indicator_limit_in_timeframe":"one_day","sell_price_limit_in_ticker_id":18,"sell_price_drop_limit_in_ticker_id":18,"sell_moving_average_limit_in_ticker_id":18,"sell_indicator_limit_in_ticker_id":18}', '2026-11-16 07:39:06.022421', '2026-03-02 14:30:00', 1, NULL, NULL, '{"missed_quote_amount":0,"missed_quote_amount_was_set":null}', 'Bots::DcaMultiAsset', '2026-11-16 07:39:06.022474', 1);
INSERT INTO "bots" (rowid, "id", "account_balance", "created_at", "current_delay", "delay", "exchange_id", "fetch_restarts", "label", "last_end_of_funds_notification", "position", "redeploy_declined_offset", "restarts", "restatement_generation", "settings", "settings_changed_at", "started_at", "status", "stop_message_key", "stopped_at", "transient_data", "type", "updated_at", "user_id") VALUES (3, 3, 0, '2026-11-16 07:39:06.032702', 0, 0, 1, 0, 'empty', NULL, 3, 0, 0, 0, '{"quote_asset_id":1,"quote_amount":100.0,"interval":"week","weighting":"manual","allocations":{"4":1.0},"smart_intervaled":false,"smart_interval_quote_amount":10.0,"limit_ordered":false,"limit_order_pcnt_distance":0.001,"quote_amount_limited":false,"quote_amount_limit":1000,"price_limited":false,"price_limit":1000000,"price_limit_range_lower_bound":0,"price_limit_range_upper_bound":1000000,"price_limit_timing_condition":"while","price_limit_value_condition":"below","price_limit_in_ticker_id":3,"price_drop_limited":false,"price_drop_limit":0.2,"price_drop_limit_time_window_condition":"ath","price_drop_limit_in_ticker_id":3,"moving_average_limited":false,"moving_average_limit_timing_condition":"while","moving_average_limit_value_condition":"below","moving_average_limit_in_ticker_id":3,"moving_average_limit_in_ma_type":"sma","moving_average_limit_in_timeframe":"one_day","moving_average_limit_in_period":9,"indicator_limited":false,"indicator_limit":30,"indicator_limit_timing_condition":"while","indicator_limit_value_condition":"below","indicator_limit_in_ticker_id":3,"indicator_limit_in_indicator":"rsi","indicator_limit_in_timeframe":"one_day","sell_price_limit_in_ticker_id":3,"sell_price_drop_limit_in_ticker_id":3,"sell_moving_average_limit_in_ticker_id":3,"sell_indicator_limit_in_ticker_id":3}', '2026-11-16 07:39:06.032647', '2026-03-02 14:30:00', 1, NULL, NULL, '{"missed_quote_amount":0,"missed_quote_amount_was_set":null}', 'Bots::DcaMultiAsset', '2026-11-16 07:39:06.032702', 1);
INSERT INTO "bots" (rowid, "id", "account_balance", "created_at", "current_delay", "delay", "exchange_id", "fetch_restarts", "label", "last_end_of_funds_notification", "position", "redeploy_declined_offset", "restarts", "restatement_generation", "settings", "settings_changed_at", "started_at", "status", "stop_message_key", "stopped_at", "transient_data", "type", "updated_at", "user_id") VALUES (4, 4, 0, '2026-11-16 07:39:06.040200', 0, 0, 1, 0, 'gone', NULL, 4, 0, 0, 0, '{"quote_asset_id":1,"quote_amount":100.0,"interval":"week","weighting":"manual","allocations":{"5":1.0},"smart_intervaled":false,"smart_interval_quote_amount":10.0,"limit_ordered":false,"limit_order_pcnt_distance":0.001,"quote_amount_limited":false,"quote_amount_limit":1000,"price_limited":false,"price_limit":1000000,"price_limit_range_lower_bound":0,"price_limit_range_upper_bound":1000000,"price_limit_timing_condition":"while","price_limit_value_condition":"below","price_limit_in_ticker_id":4,"price_drop_limited":false,"price_drop_limit":0.2,"price_drop_limit_time_window_condition":"ath","price_drop_limit_in_ticker_id":4,"moving_average_limited":false,"moving_average_limit_timing_condition":"while","moving_average_limit_value_condition":"below","moving_average_limit_in_ticker_id":4,"moving_average_limit_in_ma_type":"sma","moving_average_limit_in_timeframe":"one_day","moving_average_limit_in_period":9,"indicator_limited":false,"indicator_limit":30,"indicator_limit_timing_condition":"while","indicator_limit_value_condition":"below","indicator_limit_in_ticker_id":4,"indicator_limit_in_indicator":"rsi","indicator_limit_in_timeframe":"one_day","sell_price_limit_in_ticker_id":4,"sell_price_drop_limit_in_ticker_id":4,"sell_moving_average_limit_in_ticker_id":4,"sell_indicator_limit_in_ticker_id":4}', '2026-11-16 07:39:06.040147', '2026-03-02 14:30:00', 3, NULL, NULL, '{"missed_quote_amount":0,"missed_quote_amount_was_set":null}', 'Bots::DcaMultiAsset', '2026-11-16 07:39:06.040200', 1);
INSERT INTO "bots" (rowid, "id", "account_balance", "created_at", "current_delay", "delay", "exchange_id", "fetch_restarts", "label", "last_end_of_funds_notification", "position", "redeploy_declined_offset", "restarts", "restatement_generation", "settings", "settings_changed_at", "started_at", "status", "stop_message_key", "stopped_at", "transient_data", "type", "updated_at", "user_id") VALUES (5, 5, 0, '2026-11-16 07:39:06.056749', 0, 0, 1, 0, 'shelved', NULL, 5, 0, 0, 0, '{"quote_asset_id":1,"quote_amount":100.0,"interval":"week","weighting":"manual","allocations":{"3":1.0},"smart_intervaled":false,"smart_interval_quote_amount":10.0,"limit_ordered":false,"limit_order_pcnt_distance":0.001,"quote_amount_limited":false,"quote_amount_limit":1000,"price_limited":false,"price_limit":1000000,"price_limit_range_lower_bound":0,"price_limit_range_upper_bound":1000000,"price_limit_timing_condition":"while","price_limit_value_condition":"below","price_limit_in_ticker_id":2,"price_drop_limited":false,"price_drop_limit":0.2,"price_drop_limit_time_window_condition":"ath","price_drop_limit_in_ticker_id":2,"moving_average_limited":false,"moving_average_limit_timing_condition":"while","moving_average_limit_value_condition":"below","moving_average_limit_in_ticker_id":2,"moving_average_limit_in_ma_type":"sma","moving_average_limit_in_timeframe":"one_day","moving_average_limit_in_period":9,"indicator_limited":false,"indicator_limit":30,"indicator_limit_timing_condition":"while","indicator_limit_value_condition":"below","indicator_limit_in_ticker_id":2,"indicator_limit_in_indicator":"rsi","indicator_limit_in_timeframe":"one_day","sell_price_limit_in_ticker_id":2,"sell_price_drop_limit_in_ticker_id":2,"sell_moving_average_limit_in_ticker_id":2,"sell_indicator_limit_in_ticker_id":2}', '2026-11-16 07:39:06.056660', '2026-03-02 14:30:00', 7, NULL, NULL, '{"missed_quote_amount":0,"missed_quote_amount_was_set":null}', 'Bots::DcaMultiAsset', '2026-11-16 07:39:06.056749', 1);
CREATE TABLE "connected_clients" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "created_at" datetime(6) NOT NULL, "mcp_tools" json DEFAULT '[]', "oauth_application_id" integer NOT NULL, "rest_tools" json DEFAULT '[]', "updated_at" datetime(6) NOT NULL, "user_id" integer NOT NULL, CONSTRAINT "fk_rails_bfae147170"
FOREIGN KEY ("oauth_application_id")
  REFERENCES "oauth_applications" ("id")
 ON DELETE CASCADE, CONSTRAINT "fk_rails_cc7e236a30"
FOREIGN KEY ("user_id")
  REFERENCES "users" ("id")
);
CREATE TABLE "exchange_assets" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "asset_id" bigint NOT NULL, "available" boolean DEFAULT TRUE, "created_at" datetime(6) NOT NULL, "exchange_id" bigint NOT NULL, "updated_at" datetime(6) NOT NULL, "withdrawal_chains" json, "withdrawal_fee" varchar, "withdrawal_fee_updated_at" datetime(6), CONSTRAINT "fk_rails_a579cc966e"
FOREIGN KEY ("asset_id")
  REFERENCES "assets" ("id")
, CONSTRAINT "fk_rails_eb1e7ee730"
FOREIGN KEY ("exchange_id")
  REFERENCES "exchanges" ("id")
);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (1, 1, 1, 1, '2026-11-16 07:39:01.515687', 1, '2026-11-16 07:39:01.515687', NULL, NULL, NULL);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (2, 2, 2, 1, '2026-11-16 07:39:01.516376', 1, '2026-11-16 07:39:01.516376', NULL, NULL, NULL);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (3, 3, 3, 1, '2026-11-16 07:39:01.516847', 1, '2026-11-16 07:39:01.516847', NULL, NULL, NULL);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (4, 4, 4, 1, '2026-11-16 07:39:01.517309', 1, '2026-11-16 07:39:01.517309', NULL, NULL, NULL);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (5, 5, 5, 1, '2026-11-16 07:39:01.517764', 1, '2026-11-16 07:39:01.517764', NULL, NULL, NULL);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (6, 6, 6, 1, '2026-11-16 07:39:01.518190', 1, '2026-11-16 07:39:01.518190', NULL, NULL, NULL);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (7, 7, 7, 1, '2026-11-16 07:39:01.518623', 1, '2026-11-16 07:39:01.518623', NULL, NULL, NULL);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (8, 8, 8, 1, '2026-11-16 07:39:01.519060', 1, '2026-11-16 07:39:01.519060', NULL, NULL, NULL);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (9, 9, 9, 1, '2026-11-16 07:39:01.519492', 1, '2026-11-16 07:39:01.519492', NULL, NULL, NULL);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (10, 10, 10, 1, '2026-11-16 07:39:01.519931', 1, '2026-11-16 07:39:01.519931', NULL, NULL, NULL);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (11, 11, 11, 1, '2026-11-16 07:39:01.520373', 1, '2026-11-16 07:39:01.520373', NULL, NULL, NULL);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (12, 12, 12, 1, '2026-11-16 07:39:01.520800', 1, '2026-11-16 07:39:01.520800', NULL, NULL, NULL);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (13, 13, 13, 1, '2026-11-16 07:39:01.521231', 1, '2026-11-16 07:39:01.521231', NULL, NULL, NULL);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (14, 14, 14, 1, '2026-11-16 07:39:01.521679', 1, '2026-11-16 07:39:01.521679', NULL, NULL, NULL);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (15, 15, 15, 1, '2026-11-16 07:39:01.522106', 1, '2026-11-16 07:39:01.522106', NULL, NULL, NULL);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (16, 16, 16, 1, '2026-11-16 07:39:01.522525', 1, '2026-11-16 07:39:01.522525', NULL, NULL, NULL);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (17, 17, 17, 1, '2026-11-16 07:39:01.522951', 1, '2026-11-16 07:39:01.522951', NULL, NULL, NULL);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (18, 18, 18, 1, '2026-11-16 07:39:01.523378', 1, '2026-11-16 07:39:01.523378', NULL, NULL, NULL);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (19, 19, 19, 1, '2026-11-16 07:39:01.523799', 1, '2026-11-16 07:39:01.523799', NULL, NULL, NULL);
CREATE TABLE "fee_api_keys" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "exchange_id" bigint NOT NULL, "key" varchar, "passphrase" varchar, "secret" varchar, CONSTRAINT "fk_rails_8ea676a1dd"
FOREIGN KEY ("exchange_id")
  REFERENCES "exchanges" ("id")
);
CREATE TABLE "fund_classifications" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "created_at" datetime(6) NOT NULL, "fund_category" integer, "isin" varchar, "kind" integer NOT NULL, "symbol" varchar NOT NULL, "updated_at" datetime(6) NOT NULL, "user_id" integer NOT NULL, CONSTRAINT "fk_rails_a0cdc917d1"
FOREIGN KEY ("user_id")
  REFERENCES "users" ("id")
);
CREATE TABLE "idempotency_keys" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "created_at" datetime(6) NOT NULL, "key" varchar NOT NULL, "locked_at" datetime(6) NOT NULL, "request_fingerprint" varchar NOT NULL, "response_body" text, "response_status" integer, "state" varchar DEFAULT 'in_progress' NOT NULL, "updated_at" datetime(6) NOT NULL, "user_id" integer NOT NULL, CONSTRAINT "fk_rails_96c4cbd0a9"
FOREIGN KEY ("user_id")
  REFERENCES "users" ("id")
);
CREATE TABLE "oauth_applications" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "confidential" boolean DEFAULT FALSE NOT NULL, "created_at" datetime(6) NOT NULL, "grant_types" varchar DEFAULT 'authorization_code', "name" varchar NOT NULL, "personal_access_token" boolean DEFAULT FALSE NOT NULL, "personal_owner_id" integer, "redirect_uri" text, "registration_access_token" varchar, "response_types" varchar DEFAULT 'code', "scopes" varchar DEFAULT '' NOT NULL, "secret" varchar, "token_endpoint_auth_method" varchar DEFAULT 'none', "uid" varchar NOT NULL, "updated_at" datetime(6) NOT NULL, CONSTRAINT "fk_rails_8a5753f0c3"
FOREIGN KEY ("personal_owner_id")
  REFERENCES "users" ("id")
);
CREATE TABLE "rule_logs" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "created_at" datetime(6) NOT NULL, "details" json DEFAULT '{}' NOT NULL, "message" varchar, "rule_id" integer NOT NULL, "status" integer DEFAULT 0 NOT NULL, CONSTRAINT "fk_rails_7035e950ff"
FOREIGN KEY ("rule_id")
  REFERENCES "rules" ("id")
);
CREATE TABLE "rules" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "address" varchar, "asset_id" integer, "created_at" datetime(6) NOT NULL, "exchange_id" integer, "settings" json DEFAULT '{}' NOT NULL, "settings_changed_at" datetime(6), "status" integer DEFAULT 0 NOT NULL, "type" varchar NOT NULL, "updated_at" datetime(6) NOT NULL, "user_id" integer NOT NULL, CONSTRAINT "fk_rails_06866b945b"
FOREIGN KEY ("exchange_id")
  REFERENCES "exchanges" ("id")
, CONSTRAINT "fk_rails_d61e693d2f"
FOREIGN KEY ("asset_id")
  REFERENCES "assets" ("id")
, CONSTRAINT "fk_rails_f5e7d217a1"
FOREIGN KEY ("user_id")
  REFERENCES "users" ("id")
);
CREATE TABLE "tickers" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "ath" decimal, "ath_updated_at" datetime, "available" boolean DEFAULT TRUE, "base" varchar NOT NULL, "base_asset_id" bigint NOT NULL, "base_decimals" integer NOT NULL, "created_at" datetime(6) NOT NULL, "exchange_id" bigint NOT NULL, "maximum_base_size" decimal, "maximum_quote_size" decimal, "minimum_base_size" decimal NOT NULL, "minimum_quote_size" decimal NOT NULL, "price_decimals" integer NOT NULL, "quote" varchar NOT NULL, "quote_asset_id" bigint NOT NULL, "quote_decimals" integer NOT NULL, "ticker" varchar NOT NULL, "trading_enabled" boolean DEFAULT TRUE NOT NULL, "updated_at" datetime(6) NOT NULL, CONSTRAINT "fk_rails_3d0bf94439"
FOREIGN KEY ("quote_asset_id")
  REFERENCES "assets" ("id")
, CONSTRAINT "fk_rails_f8d93e4e0f"
FOREIGN KEY ("base_asset_id")
  REFERENCES "assets" ("id")
, CONSTRAINT "fk_rails_a0c7510cbd"
FOREIGN KEY ("exchange_id")
  REFERENCES "exchanges" ("id")
);
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (1, 1, NULL, NULL, 1, 'AAA', 2, 9, '2026-11-16 07:39:01.553021', 1, NULL, NULL, 1e-9, 1, 2, 'USD', 1, 2, 'AAA', 1, '2026-11-16 07:39:01.553021');
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (2, 2, NULL, NULL, 1, 'BBB', 3, 9, '2026-11-16 07:39:01.554437', 1, NULL, NULL, 1e-9, 1, 2, 'USD', 1, 2, 'BBB', 1, '2026-11-16 07:39:01.554437');
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (3, 3, NULL, NULL, 1, 'CCC', 4, 9, '2026-11-16 07:39:01.555365', 1, NULL, NULL, 1e-9, 1, 2, 'USD', 1, 2, 'CCC', 1, '2026-11-16 07:39:01.555365');
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (4, 4, NULL, NULL, 1, 'DDD', 5, 9, '2026-11-16 07:39:01.556287', 1, NULL, NULL, 1e-9, 1, 2, 'USD', 1, 2, 'DDD', 1, '2026-11-16 07:39:01.556287');
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (5, 5, NULL, NULL, 1, 'EEE', 6, 9, '2026-11-16 07:39:01.557199', 1, NULL, NULL, 1e-9, 1, 2, 'USD', 1, 2, 'EEE', 1, '2026-11-16 07:39:01.557199');
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (6, 6, NULL, NULL, 1, 'FFF', 7, 9, '2026-11-16 07:39:01.558117', 1, NULL, NULL, 1e-9, 1, 2, 'USD', 1, 2, 'FFF', 1, '2026-11-16 07:39:01.558117');
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (7, 7, NULL, NULL, 1, 'GGG', 8, 9, '2026-11-16 07:39:01.559010', 1, NULL, NULL, 1e-9, 1, 2, 'USD', 1, 2, 'GGG', 1, '2026-11-16 07:39:01.559010');
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (8, 8, NULL, NULL, 1, 'HHH', 9, 9, '2026-11-16 07:39:01.559906', 1, NULL, NULL, 1e-9, 1, 2, 'USD', 1, 2, 'HHH', 1, '2026-11-16 07:39:01.559906');
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (9, 9, NULL, NULL, 1, 'III', 10, 9, '2026-11-16 07:39:01.560788', 1, NULL, NULL, 1e-9, 1, 2, 'USD', 1, 2, 'III', 1, '2026-11-16 07:39:01.560788');
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (10, 10, NULL, NULL, 1, 'JJJ', 11, 9, '2026-11-16 07:39:01.561673', 1, NULL, NULL, 1e-9, 1, 2, 'USD', 1, 2, 'JJJ', 1, '2026-11-16 07:39:01.561673');
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (11, 11, NULL, NULL, 1, 'KKK', 12, 9, '2026-11-16 07:39:01.562561', 1, NULL, NULL, 1e-9, 1, 2, 'USD', 1, 2, 'KKK', 1, '2026-11-16 07:39:01.562561');
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (12, 12, NULL, NULL, 1, 'LLL', 13, 9, '2026-11-16 07:39:01.563460', 1, NULL, NULL, 1e-9, 1, 2, 'USD', 1, 2, 'LLL', 1, '2026-11-16 07:39:01.563460');
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (13, 13, NULL, NULL, 1, 'AAA.X', 17, 9, '2026-11-16 07:39:01.564363', 1, NULL, NULL, 1e-9, 1, 2, 'USD', 1, 2, 'AAA.X', 1, '2026-11-16 07:39:01.564363');
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (14, 14, NULL, NULL, 1, 'NMD', 18, 9, '2026-11-16 07:39:01.565262', 1, NULL, NULL, 1e-9, 1, 2, 'USD', 1, 2, 'NMD', 1, '2026-11-16 07:39:01.565262');
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (15, 15, NULL, NULL, 1, 'BRE', 19, 9, '2026-11-16 07:39:01.566154', 1, NULL, NULL, 1e-9, 1, 2, 'USD', 1, 2, 'BRE', 1, '2026-11-16 07:39:01.566154');
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (16, 16, NULL, NULL, 1, 'BTC', 14, 9, '2026-11-16 07:39:01.567038', 1, NULL, NULL, 1e-9, 1, 2, 'USD', 1, 4, 'BTC/USD', 1, '2026-11-16 07:39:01.567038');
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (17, 17, NULL, NULL, 1, 'ETH', 15, 9, '2026-11-16 07:39:01.567972', 1, NULL, NULL, 1e-9, 1, 2, 'USD', 1, 4, 'ETH/USD', 1, '2026-11-16 07:39:01.567972');
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (18, 18, NULL, NULL, 1, 'BTC', 14, 9, '2026-11-16 07:39:01.568928', 1, NULL, NULL, 1e-9, 1, 2, 'USDT', 16, 6, 'BTC/USDT', 1, '2026-11-16 07:39:01.568928');
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (19, 19, NULL, NULL, 1, 'ETH', 15, 9, '2026-11-16 07:39:01.569820', 1, NULL, NULL, 1e-9, 1, 2, 'USDT', 16, 6, 'ETH/USDT', 1, '2026-11-16 07:39:01.569820');
CREATE TABLE "transactions" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "amount" decimal, "amount_exec" decimal, "base" varchar, "base_asset_id" integer, "bot_id" bigint, "bot_interval" varchar DEFAULT '' NOT NULL, "bot_quote_amount" decimal DEFAULT 0.0 NOT NULL, "created_at" datetime NOT NULL, "error_messages" json DEFAULT '[]' NOT NULL, "exchange_id" bigint NOT NULL, "external_id" varchar, "external_status" integer, "order_type" integer, "price" decimal, "quote" varchar, "quote_amount" decimal, "quote_amount_exec" decimal, "quote_asset_id" integer, "side" integer, "status" integer, "transaction_type" varchar DEFAULT 'REGULAR' NOT NULL, "updated_at" datetime NOT NULL, "run_identity" json, CONSTRAINT "fk_rails_d4bc8411ec"
FOREIGN KEY ("bot_id")
  REFERENCES "bots" ("id")
, CONSTRAINT "fk_rails_e7352b1d47"
FOREIGN KEY ("exchange_id")
  REFERENCES "exchanges" ("id")
);
INSERT INTO "transactions" (rowid, "id", "amount", "amount_exec", "base", "base_asset_id", "bot_id", "bot_interval", "bot_quote_amount", "created_at", "error_messages", "exchange_id", "external_id", "external_status", "order_type", "price", "quote", "quote_amount", "quote_amount_exec", "quote_asset_id", "side", "status", "transaction_type", "updated_at", "run_identity") VALUES (1, 1, 0.591891092, 0.591891092, 'AAA', 2, 1, 'week', 100, '2026-03-02 14:30:00', '[]', 1, 'usd-0', 2, 0, 101.37, 'USD', NULL, 59.99999999604, 1, 0, 0, 'REGULAR', '2026-03-02 14:30:00', NULL);
INSERT INTO "transactions" (rowid, "id", "amount", "amount_exec", "base", "base_asset_id", "bot_id", "bot_interval", "bot_quote_amount", "created_at", "error_messages", "exchange_id", "external_id", "external_status", "order_type", "price", "quote", "quote_amount", "quote_amount_exec", "quote_asset_id", "side", "status", "transaction_type", "updated_at", "run_identity") VALUES (2, 2, 0.72450643, 0.72450643, 'BBB', 3, 1, 'week', 100, '2026-03-02 14:30:01', '[]', 1, 'usd-1', 2, 0, 55.21, 'USD', NULL, 40.0000000003, 1, 0, 0, 'REGULAR', '2026-03-02 14:30:01', NULL);
INSERT INTO "transactions" (rowid, "id", "amount", "amount_exec", "base", "base_asset_id", "bot_id", "bot_interval", "bot_quote_amount", "created_at", "error_messages", "exchange_id", "external_id", "external_status", "order_type", "price", "quote", "quote_amount", "quote_amount_exec", "quote_asset_id", "side", "status", "transaction_type", "updated_at", "run_identity") VALUES (3, 3, 0.581902822, 0.581902822, 'AAA', 2, 1, 'week', 100, '2026-03-09 14:30:00', '[]', 1, 'usd-2', 2, 0, 103.11, 'USD', NULL, 59.99999997642, 1, 0, 0, 'REGULAR', '2026-03-09 14:30:00', NULL);
INSERT INTO "transactions" (rowid, "id", "amount", "amount_exec", "base", "base_asset_id", "bot_id", "bot_interval", "bot_quote_amount", "created_at", "error_messages", "exchange_id", "external_id", "external_status", "order_type", "price", "quote", "quote_amount", "quote_amount_exec", "quote_asset_id", "side", "status", "transaction_type", "updated_at", "run_identity") VALUES (4, 4, 0.740466494, 0.740466494, 'BBB', 3, 1, 'week', 100, '2026-03-09 14:30:01', '[]', 1, 'usd-3', 2, 0, 54.02, 'USD', NULL, 40.00000000588, 1, 0, 0, 'REGULAR', '2026-03-09 14:30:01', NULL);
INSERT INTO "transactions" (rowid, "id", "amount", "amount_exec", "base", "base_asset_id", "bot_id", "bot_interval", "bot_quote_amount", "created_at", "error_messages", "exchange_id", "external_id", "external_status", "order_type", "price", "quote", "quote_amount", "quote_amount_exec", "quote_asset_id", "side", "status", "transaction_type", "updated_at", "run_identity") VALUES (5, 5, 0.000937498, 0.000937498, 'BTC', 14, 2, 'week', 100, '2026-03-02 14:30:00', '[]', 1, 'usdt-0', 2, 0, 64000.12, 'USDT', NULL, 59.99998449976, 16, 0, 0, 'REGULAR', '2026-03-02 14:30:00', NULL);
INSERT INTO "transactions" (rowid, "id", "amount", "amount_exec", "base", "base_asset_id", "bot_id", "bot_interval", "bot_quote_amount", "created_at", "error_messages", "exchange_id", "external_id", "external_status", "order_type", "price", "quote", "quote_amount", "quote_amount_exec", "quote_asset_id", "side", "status", "transaction_type", "updated_at", "run_identity") VALUES (6, 6, 0.012901001, 0.012901001, 'ETH', 15, 2, 'week', 100, '2026-03-02 14:30:01', '[]', 1, 'usdt-1', 2, 0, 3100.55, 'USDT', NULL, 40.00019865055, 16, 0, 0, 'REGULAR', '2026-03-02 14:30:01', NULL);
INSERT INTO "transactions" (rowid, "id", "amount", "amount_exec", "base", "base_asset_id", "bot_id", "bot_interval", "bot_quote_amount", "created_at", "error_messages", "exchange_id", "external_id", "external_status", "order_type", "price", "quote", "quote_amount", "quote_amount_exec", "quote_asset_id", "side", "status", "transaction_type", "updated_at", "run_identity") VALUES (7, 7, 0.000922928, 0.000922928, 'BTC', 14, 2, 'week', 100, '2026-03-09 14:30:00', '[]', 1, 'usdt-2', 2, 0, 65010.5, 'USDT', NULL, 60.000010744, 16, 0, 0, 'REGULAR', '2026-03-09 14:30:00', NULL);
INSERT INTO "transactions" (rowid, "id", "amount", "amount_exec", "base", "base_asset_id", "bot_id", "bot_interval", "bot_quote_amount", "created_at", "error_messages", "exchange_id", "external_id", "external_status", "order_type", "price", "quote", "quote_amount", "quote_amount_exec", "quote_asset_id", "side", "status", "transaction_type", "updated_at", "run_identity") VALUES (8, 8, 0.013093246, 0.013093246, 'ETH', 15, 2, 'week', 100, '2026-03-09 14:30:01', '[]', 1, 'usdt-3', 2, 0, 3055.01, 'USDT', NULL, 39.99999746246, 16, 0, 0, 'REGULAR', '2026-03-09 14:30:01', NULL);
INSERT INTO "transactions" (rowid, "id", "amount", "amount_exec", "base", "base_asset_id", "bot_id", "bot_interval", "bot_quote_amount", "created_at", "error_messages", "exchange_id", "external_id", "external_status", "order_type", "price", "quote", "quote_amount", "quote_amount_exec", "quote_asset_id", "side", "status", "transaction_type", "updated_at", "run_identity") VALUES (9, 9, 1, 1, 'DDD', 5, 4, 'week', 100, '2026-03-02 14:30:00', '[]', 1, 'gone-0', 2, 0, 80, 'USD', NULL, 80, 1, 0, 0, 'REGULAR', '2026-03-02 14:30:00', NULL);
INSERT INTO "transactions" (rowid, "id", "amount", "amount_exec", "base", "base_asset_id", "bot_id", "bot_interval", "bot_quote_amount", "created_at", "error_messages", "exchange_id", "external_id", "external_status", "order_type", "price", "quote", "quote_amount", "quote_amount_exec", "quote_asset_id", "side", "status", "transaction_type", "updated_at", "run_identity") VALUES (10, 10, 2, 2, 'BBB', 3, 5, 'week', 100, '2026-03-03 14:30:00', '[]', 1, 'shelved-0', 2, 0, 55, 'USD', NULL, 110, 1, 0, 0, 'REGULAR', '2026-03-03 14:30:00', NULL);
CREATE TABLE "wash_sale_locks" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "asset_id" integer NOT NULL, "buy_locked_until" datetime(6), "claim_token" varchar, "confirmed_locked_until" datetime(6), "created_at" datetime(6) NOT NULL, "source" varchar DEFAULT 'bot' NOT NULL, "updated_at" datetime(6) NOT NULL, "user_id" integer NOT NULL, CONSTRAINT "fk_rails_52739acead"
FOREIGN KEY ("asset_id")
  REFERENCES "assets" ("id")
, CONSTRAINT "fk_rails_482271be5e"
FOREIGN KEY ("user_id")
  REFERENCES "users" ("id")
);
CREATE TABLE "schema_migrations" ("version" varchar NOT NULL PRIMARY KEY);
INSERT INTO "schema_migrations" (rowid, "version") VALUES (1, '20261017100000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (2, '20261016140000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (3, '20261016130000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (4, '20261016120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (5, '20261015120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (6, '20261014120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (7, '20261011170000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (8, '20261009130000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (9, '20261009120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (10, '20261006100000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (11, '20260929120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (12, '20260928120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (13, '20260921180000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (14, '20260921120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (15, '20260918150000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (16, '20260918120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (17, '20260918090000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (18, '20260908130000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (19, '20260908120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (20, '20260908091000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (21, '20260908090000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (22, '20260907110000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (23, '20260907100000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (24, '20260905190000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (25, '20260830230000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (26, '20260830210000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (27, '20260830120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (28, '20260829132450');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (29, '20260828175918');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (30, '20260828095813');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (31, '20260828094446');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (32, '20260826140000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (33, '20260826090000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (34, '20260825200000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (35, '20260825120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (36, '20260825060000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (37, '20260824140000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (38, '20260824130000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (39, '20260824120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (40, '20260823120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (41, '20260822100000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (42, '20260821220432');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (43, '20260821120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (44, '20260818120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (45, '20260815170100');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (46, '20260815170000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (47, '20260815160000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (48, '20260815150100');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (49, '20260815150060');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (50, '20260815150050');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (51, '20260815150000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (52, '20260815140000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (53, '20260815130000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (54, '20260815120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (55, '20260810120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (56, '20260808230550');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (57, '20260808225248');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (58, '20260806001012');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (59, '20260727120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (60, '20260706170000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (61, '20260624081139');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (62, '20260603234747');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (63, '20260603231515');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (64, '20260603222643');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (65, '20260530000000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (66, '20260529001000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (67, '20260527002000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (68, '20260527001000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (69, '20260527000000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (70, '20260524000000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (71, '20260520000000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (72, '20260413150000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (73, '20260413140000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (74, '20260413120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (75, '20260325221615');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (76, '20260325183549');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (77, '20260324132010');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (78, '20260323052902');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (79, '20260322140051');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (80, '20260320042623');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (81, '20260316000001');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (82, '20260311145662');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (83, '20260311145661');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (84, '20260311145660');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (85, '20260311145659');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (86, '20260311145658');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (87, '20260311145657');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (88, '20260311145656');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (89, '20260311145655');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (90, '20260311145654');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (91, '20260308090843');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (92, '20260228152720');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (93, '20260228145540');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (94, '20260228130833');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (95, '20260227171710');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (96, '20260226000000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (97, '20260224065811');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (98, '20260219130000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (99, '20260219120000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (100, '20260219110000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (101, '20260219100000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (102, '20260217100001');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (103, '20260217100000');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (104, '20260208114731');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (105, '20260203191449');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (106, '20260130114825');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (107, '20260129142740');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (108, '20260129111946');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (109, '20260128000001');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (110, '20260127164433');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (111, '20260127112431');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (112, '20260126145012');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (113, '20260126141242');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (114, '20260114154223');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (115, '20260114145256');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (116, '20260109050047');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (117, '20260106162140');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (118, '20260106115209');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (119, '20260106111302');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (120, '20260106111301');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (121, '20260104194208');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (122, '20260103000001');
INSERT INTO "schema_migrations" (rowid, "version") VALUES (123, '20260102171555');
CREATE TABLE "ar_internal_metadata" ("key" varchar NOT NULL PRIMARY KEY, "value" varchar, "created_at" datetime(6) NOT NULL, "updated_at" datetime(6) NOT NULL);
INSERT INTO "ar_internal_metadata" (rowid, "key", "value", "created_at", "updated_at") VALUES (1, 'environment', 'production', '2026-11-16 07:39:01.221', '2026-11-16 07:39:01.221');
DELETE FROM sqlite_sequence;
INSERT INTO sqlite_sequence (name, seq) VALUES ('account_balances', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('account_transactions', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('action_mcp_session_messages', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('action_mcp_session_subscriptions', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('api_keys', 1);
INSERT INTO sqlite_sequence (name, seq) VALUES ('bot_activity_logs', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('bot_index_assets', 7);
INSERT INTO sqlite_sequence (name, seq) VALUES ('bot_signals', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('bots', 5);
INSERT INTO sqlite_sequence (name, seq) VALUES ('connected_clients', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('exchange_assets', 19);
INSERT INTO sqlite_sequence (name, seq) VALUES ('fee_api_keys', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('fund_classifications', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('idempotency_keys', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('oauth_applications', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('rule_logs', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('rules', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('tickers', 19);
INSERT INTO sqlite_sequence (name, seq) VALUES ('transactions', 10);
INSERT INTO sqlite_sequence (name, seq) VALUES ('wash_sale_locks', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('users', 1);
INSERT INTO sqlite_sequence (name, seq) VALUES ('exchanges', 1);
INSERT INTO sqlite_sequence (name, seq) VALUES ('assets', 19);
INSERT INTO sqlite_sequence (name, seq) VALUES ('app_configs', 3);
CREATE UNIQUE INDEX "index_app_configs_on_key" ON "app_configs" ("key");
CREATE UNIQUE INDEX "index_assets_on_external_id" ON "assets" ("external_id");
CREATE INDEX "index_assets_on_isin" ON "assets" ("isin");
CREATE INDEX "index_assets_on_name" ON "assets" ("name");
CREATE INDEX "index_assets_on_symbol" ON "assets" ("symbol");
CREATE UNIQUE INDEX "index_exchanges_on_type" ON "exchanges" ("type");
CREATE UNIQUE INDEX "index_fx_rates_on_currency_and_date" ON "fx_rates" ("currency", "date");
CREATE UNIQUE INDEX "index_historical_prices_on_asset_and_currency_and_date" ON "historical_prices" ("asset", "currency", "date");
CREATE INDEX "index_ibkr_locks_on_expires_at" ON "ibkr_locks" ("expires_at");
CREATE UNIQUE INDEX "index_ibkr_locks_on_key" ON "ibkr_locks" ("key");
CREATE UNIQUE INDEX "index_indices_on_external_id_and_source" ON "indices" ("external_id", "source");
CREATE INDEX "index_indices_on_weight" ON "indices" ("weight");
CREATE INDEX "index_oauth_access_grants_on_application_id" ON "oauth_access_grants" ("application_id");
CREATE INDEX "index_oauth_access_grants_on_resource_owner_id" ON "oauth_access_grants" ("resource_owner_id");
CREATE UNIQUE INDEX "index_oauth_access_grants_on_token" ON "oauth_access_grants" ("token");
CREATE INDEX "index_oauth_access_tokens_on_application_id" ON "oauth_access_tokens" ("application_id");
CREATE UNIQUE INDEX "index_oauth_access_tokens_on_refresh_token" ON "oauth_access_tokens" ("refresh_token");
CREATE INDEX "index_oauth_access_tokens_on_resource_owner_id" ON "oauth_access_tokens" ("resource_owner_id");
CREATE UNIQUE INDEX "index_oauth_access_tokens_on_token" ON "oauth_access_tokens" ("token");
CREATE UNIQUE INDEX "index_portfolio_snapshots_on_user_id_and_date" ON "portfolio_snapshots" ("user_id", "date");
CREATE INDEX "index_portfolio_snapshots_on_user_id" ON "portfolio_snapshots" ("user_id");
CREATE UNIQUE INDEX "idx_on_user_id_exchange_id_date_16fb7188ca" ON "portfolio_venue_snapshots" ("user_id", "exchange_id", "date");
CREATE UNIQUE INDEX "index_users_on_confirmation_token" ON "users" ("confirmation_token");
CREATE UNIQUE INDEX "index_users_on_email" ON "users" ("email");
CREATE UNIQUE INDEX "index_users_on_reset_password_token" ON "users" ("reset_password_token");
CREATE INDEX "index_account_balances_on_asset_id" ON "account_balances" ("asset_id");
CREATE INDEX "index_account_balances_on_exchange_id" ON "account_balances" ("exchange_id");
CREATE UNIQUE INDEX "idx_account_balances_user_exchange_asset" ON "account_balances" ("user_id", "exchange_id", "asset_id");
CREATE INDEX "index_account_balances_on_user_id" ON "account_balances" ("user_id");
CREATE INDEX "index_account_transactions_on_api_key_id_and_transacted_at" ON "account_transactions" ("api_key_id", "transacted_at");
CREATE INDEX "index_account_transactions_on_api_key_id" ON "account_transactions" ("api_key_id");
CREATE INDEX "index_account_transactions_on_exchange_id" ON "account_transactions" ("exchange_id");
CREATE INDEX "index_account_transactions_on_group_id" ON "account_transactions" ("group_id");
CREATE UNIQUE INDEX "index_account_transactions_on_linked_transaction_id" ON "account_transactions" ("linked_transaction_id");
CREATE INDEX "index_account_transactions_on_transacted_at" ON "account_transactions" ("transacted_at");
CREATE INDEX "index_account_transactions_on_transaction_id" ON "account_transactions" ("transaction_id");
CREATE UNIQUE INDEX "index_account_transactions_on_user_exchange_tx_id" ON "account_transactions" ("user_id", "exchange_id", "tx_id") WHERE tx_id IS NOT NULL;
CREATE INDEX "index_account_transactions_on_user_id" ON "account_transactions" ("user_id");
CREATE INDEX "index_action_mcp_session_messages_on_session_id" ON "action_mcp_session_messages" ("session_id");
CREATE INDEX "index_action_mcp_session_subscriptions_on_session_id" ON "action_mcp_session_subscriptions" ("session_id");
CREATE INDEX "index_action_mcp_session_tasks_on_created_at" ON "action_mcp_session_tasks" ("created_at");
CREATE INDEX "index_action_mcp_session_tasks_on_session_id_and_status" ON "action_mcp_session_tasks" ("session_id", "status");
CREATE INDEX "index_action_mcp_session_tasks_on_session_id" ON "action_mcp_session_tasks" ("session_id");
CREATE INDEX "index_action_mcp_session_tasks_on_status" ON "action_mcp_session_tasks" ("status");
CREATE INDEX "index_api_keys_on_exchange_id" ON "api_keys" ("exchange_id");
CREATE INDEX "index_api_keys_on_user_id" ON "api_keys" ("user_id");
CREATE INDEX "index_bot_activity_logs_on_bot_id_and_created_at" ON "bot_activity_logs" ("bot_id", "created_at");
CREATE INDEX "index_bot_activity_logs_on_bot_id" ON "bot_activity_logs" ("bot_id");
CREATE INDEX "index_bot_index_assets_on_asset_id" ON "bot_index_assets" ("asset_id");
CREATE UNIQUE INDEX "index_bot_index_assets_on_bot_id_and_asset_id" ON "bot_index_assets" ("bot_id", "asset_id");
CREATE INDEX "index_bot_index_assets_on_bot_id" ON "bot_index_assets" ("bot_id");
CREATE INDEX "index_bot_index_assets_on_ticker_id" ON "bot_index_assets" ("ticker_id");
CREATE INDEX "index_bot_signals_on_bot_id" ON "bot_signals" ("bot_id");
CREATE UNIQUE INDEX "index_bot_signals_on_token" ON "bot_signals" ("token");
CREATE INDEX "index_bots_on_exchange_id" ON "bots" ("exchange_id");
CREATE INDEX "index_bots_on_user_id" ON "bots" ("user_id");
CREATE INDEX "index_connected_clients_on_oauth_application_id" ON "connected_clients" ("oauth_application_id");
CREATE UNIQUE INDEX "index_connected_clients_on_user_and_application" ON "connected_clients" ("user_id", "oauth_application_id");
CREATE INDEX "index_connected_clients_on_user_id" ON "connected_clients" ("user_id");
CREATE UNIQUE INDEX "index_exchange_assets_on_asset_id_and_exchange_id" ON "exchange_assets" ("asset_id", "exchange_id");
CREATE INDEX "index_exchange_assets_on_asset_id" ON "exchange_assets" ("asset_id");
CREATE INDEX "index_exchange_assets_on_exchange_id" ON "exchange_assets" ("exchange_id");
CREATE INDEX "index_fee_api_keys_on_exchange_id" ON "fee_api_keys" ("exchange_id");
CREATE UNIQUE INDEX "index_fund_classifications_on_user_id_and_symbol" ON "fund_classifications" ("user_id", "symbol");
CREATE INDEX "index_idempotency_keys_on_created_at" ON "idempotency_keys" ("created_at");
CREATE UNIQUE INDEX "index_idempotency_keys_on_user_id_and_key" ON "idempotency_keys" ("user_id", "key");
CREATE INDEX "index_idempotency_keys_on_user_id" ON "idempotency_keys" ("user_id");
CREATE UNIQUE INDEX "index_oauth_applications_unique_personal_owner" ON "oauth_applications" ("personal_owner_id") WHERE personal_access_token = 1;
CREATE UNIQUE INDEX "index_oauth_applications_on_registration_access_token" ON "oauth_applications" ("registration_access_token");
CREATE UNIQUE INDEX "index_oauth_applications_on_uid" ON "oauth_applications" ("uid");
CREATE INDEX "index_rule_logs_on_rule_id_and_created_at" ON "rule_logs" ("rule_id", "created_at");
CREATE INDEX "index_rule_logs_on_rule_id" ON "rule_logs" ("rule_id");
CREATE INDEX "index_rules_on_asset_id" ON "rules" ("asset_id");
CREATE INDEX "index_rules_on_exchange_id" ON "rules" ("exchange_id");
CREATE UNIQUE INDEX "idx_rules_user_type_exchange_asset" ON "rules" ("user_id", "type", "exchange_id", "asset_id");
CREATE INDEX "index_rules_on_user_id" ON "rules" ("user_id");
CREATE INDEX "index_tickers_on_base_asset_id" ON "tickers" ("base_asset_id");
CREATE UNIQUE INDEX "index_exchange_tickers_on_unique_base_asset_and_quote_asset" ON "tickers" ("exchange_id", "base_asset_id", "quote_asset_id");
CREATE UNIQUE INDEX "index_exchange_tickers_on_unique_ticker" ON "tickers" ("exchange_id", "ticker");
CREATE INDEX "index_tickers_on_exchange_id" ON "tickers" ("exchange_id");
CREATE INDEX "index_tickers_on_quote_asset_id" ON "tickers" ("quote_asset_id");
CREATE INDEX "index_transactions_on_bot_id_and_created_at" ON "transactions" ("bot_id", "created_at");
CREATE INDEX "index_transactions_on_bot_id_and_status_and_created_at" ON "transactions" ("bot_id", "status", "created_at");
CREATE INDEX "index_bot_type_created_at" ON "transactions" ("bot_id", "transaction_type", "created_at");
CREATE INDEX "index_transactions_on_bot_id" ON "transactions" ("bot_id");
CREATE INDEX "index_transactions_on_created_at" ON "transactions" ("created_at");
CREATE UNIQUE INDEX "index_transactions_on_exchange_id_and_external_id" ON "transactions" ("exchange_id", "external_id");
CREATE INDEX "index_transactions_on_exchange_id" ON "transactions" ("exchange_id");
CREATE INDEX "index_wash_sale_locks_on_asset_id" ON "wash_sale_locks" ("asset_id");
CREATE UNIQUE INDEX "index_wash_sale_locks_on_user_id_and_asset_id" ON "wash_sale_locks" ("user_id", "asset_id");
COMMIT;
