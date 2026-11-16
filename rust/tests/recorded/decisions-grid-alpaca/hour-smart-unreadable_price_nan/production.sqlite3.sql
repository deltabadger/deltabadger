-- Rails' answer, frozen from the commit tagged rails-oracle-final (PLAN-B11 B11-1).
PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
CREATE TABLE "action_mcp_sessions" ("id" varchar NOT NULL PRIMARY KEY, "client_capabilities" json, "client_info" json, "consents" json DEFAULT '{}' NOT NULL, "created_at" datetime(6) NOT NULL, "ended_at" datetime(6), "initialized" boolean DEFAULT FALSE NOT NULL, "messages_count" integer DEFAULT 0 NOT NULL, "prompt_registry" json DEFAULT '[]', "protocol_version" varchar, "resource_registry" json DEFAULT '[]', "role" varchar DEFAULT 'server' NOT NULL, "server_capabilities" json, "server_info" json, "session_data" json DEFAULT '{}' NOT NULL, "status" varchar DEFAULT 'pre_initialize' NOT NULL, "tool_registry" json DEFAULT '[]', "updated_at" datetime(6) NOT NULL);
CREATE TABLE "app_configs" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "created_at" datetime(6) NOT NULL, "key" varchar NOT NULL, "updated_at" datetime(6) NOT NULL, "value" text);
CREATE TABLE "assets" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "category" varchar, "circulating_supply" decimal(30,8), "color" varchar, "country" varchar, "country_exchange" varchar, "created_at" datetime(6) NOT NULL, "external_id" varchar NOT NULL, "image_url" varchar, "instrument_type" varchar, "isin" varchar, "market_cap" bigint, "market_cap_rank" integer, "name" varchar, "symbol" varchar, "updated_at" datetime(6) NOT NULL, "url" varchar);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (1, 1, 'Cryptocurrency', NULL, NULL, NULL, NULL, '2026-11-16 07:43:24.572731', 'bitcoin', NULL, NULL, NULL, NULL, NULL, 'Bitcoin', 'BTC', '2026-11-16 07:43:24.572731', NULL);
INSERT INTO "assets" (rowid, "id", "category", "circulating_supply", "color", "country", "country_exchange", "created_at", "external_id", "image_url", "instrument_type", "isin", "market_cap", "market_cap_rank", "name", "symbol", "updated_at", "url") VALUES (2, 2, 'Currency', NULL, NULL, NULL, NULL, '2026-11-16 07:43:24.575057', 'usd', NULL, NULL, NULL, NULL, NULL, 'US Dollar', 'USD', '2026-11-16 07:43:24.575057', NULL);
CREATE TABLE "exchanges" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "available" boolean DEFAULT TRUE, "created_at" datetime NOT NULL, "maker_fee" varchar, "name" varchar, "taker_fee" varchar, "type" varchar, "updated_at" datetime NOT NULL);
INSERT INTO "exchanges" (rowid, "id", "available", "created_at", "maker_fee", "name", "taker_fee", "type", "updated_at") VALUES (1, 1, 1, '2026-11-16 07:43:24.570789', '0.15', 'Alpaca', '0.25', 'Exchanges::Alpaca', '2026-11-16 07:43:24.570789');
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
INSERT INTO "users" (rowid, "id", "admin", "advanced_bots_enabled", "confirmation_sent_at", "confirmation_token", "confirmed_at", "created_at", "display_currency", "email", "encrypted_password", "failed_attempts", "hide_balances", "last_otp_at", "locale", "locked_at", "mcp_settings", "name", "otp_module", "otp_secret_key", "remember_created_at", "reset_password_sent_at", "reset_password_token", "rest_settings", "setup_completed", "show_smart_intervals_info", "subscribed_to_email_marketing", "time_zone", "tracker_settings", "unconfirmed_email", "updated_at", "wash_sale_enabled", "wash_sale_jurisdiction", "wash_sale_selling_warned_at") VALUES (1, 1, 1, 0, NULL, NULL, '2026-11-16 07:43:24.564855', '2026-11-16 07:43:24.565189', 'USD', 'owner@example.com', '$2a$11$Yt7IC5LkKMV5rYplw0SaOOLZrzHjF5s/ti1KukffnE6Zds5wym08S', 0, 0, NULL, NULL, NULL, '{}', 'Owner', 0, '{"p":"hz5wfgtiPaJNloNYjcdRMHjoWbBlQDoZErxy7uamvhs=","h":{"iv":"nrLQEqQgs3Gc6gj3","at":"c0J0GGt5Tt0ASw+uVOE8Mg==","e":"QVNDSUktOEJJVA=="}}', NULL, NULL, NULL, '{}', 1, 1, 1, 'UTC', '{}', NULL, '2026-11-16 07:43:24.565189', NULL, NULL, NULL);
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
INSERT INTO "api_keys" (rowid, "id", "access_token", "balances_synced_at", "created_at", "dh_param", "exchange_id", "german_trading_agreement", "ibkr_realm", "key", "key_type", "last_sync_error", "last_synced_at", "passphrase", "rsa_encryption_key", "rsa_signature_key", "secret", "status", "updated_at", "user_id") VALUES (1, 1, NULL, NULL, '2026-11-16 07:43:24.587971', NULL, 1, NULL, NULL, '{"p":"fg==","h":{"iv":"VYxdsb481xe3Be6C","at":"z8OEhE/RR7+QTy47kSQ62g=="}}', 0, NULL, NULL, '{"p":"Ee51a3A=","h":{"iv":"Dbxmx8/+NlVdY278","at":"2FjrHXPkDslr9D310iVYdA=="}}', NULL, NULL, '{"p":"OA==","h":{"iv":"SGjNhdRsrvdfhlgK","at":"o63C0XBBhdceBHSPpxaJrw=="}}', 1, '2026-11-16 07:43:24.587971', 1);
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
INSERT INTO "bot_index_assets" (rowid, "id", "asset_id", "bot_id", "created_at", "current_allocation", "entered_at", "exited_at", "in_index", "target_allocation", "ticker_id", "updated_at") VALUES (1, 1, 1, 1, '2026-11-16 07:43:24.602787', NULL, '2026-11-16 07:43:24.599041', NULL, 1, 1, 1, '2026-11-16 07:43:24.602787');
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
INSERT INTO "bots" (rowid, "id", "account_balance", "created_at", "current_delay", "delay", "exchange_id", "fetch_restarts", "label", "last_end_of_funds_notification", "position", "redeploy_declined_offset", "restarts", "restatement_generation", "settings", "settings_changed_at", "started_at", "status", "stop_message_key", "stopped_at", "transient_data", "type", "updated_at", "user_id") VALUES (1, 1, 0, '2026-11-16 07:43:24.595369', 0, 0, 1, 0, 'Bitcoin', NULL, 1, 0, 0, 0, '{"quote_asset_id":2,"quote_amount":60.0,"interval":"hour","weighting":"manual","allocations":{"1":1.0},"smart_intervaled":true,"smart_interval_quote_amount":20.0,"limit_ordered":false,"limit_order_pcnt_distance":0.001,"quote_amount_limited":false,"quote_amount_limit":1000,"price_limited":false,"price_limit":1000000,"price_limit_range_lower_bound":0,"price_limit_range_upper_bound":1000000,"price_limit_timing_condition":"while","price_limit_value_condition":"below","price_limit_in_ticker_id":1,"price_drop_limited":false,"price_drop_limit":0.2,"price_drop_limit_time_window_condition":"ath","price_drop_limit_in_ticker_id":1,"moving_average_limited":false,"moving_average_limit_timing_condition":"while","moving_average_limit_value_condition":"below","moving_average_limit_in_ticker_id":1,"moving_average_limit_in_ma_type":"sma","moving_average_limit_in_timeframe":"one_day","moving_average_limit_in_period":9,"indicator_limited":false,"indicator_limit":30,"indicator_limit_timing_condition":"while","indicator_limit_value_condition":"below","indicator_limit_in_ticker_id":1,"indicator_limit_in_indicator":"rsi","indicator_limit_in_timeframe":"one_day","sell_price_limit_in_ticker_id":1,"sell_price_drop_limit_in_ticker_id":1,"sell_moving_average_limit_in_ticker_id":1,"sell_indicator_limit_in_ticker_id":1}', NULL, '2026-09-01 10:00:00.123456', 1, NULL, NULL, '{"missed_quote_amount":0,"missed_quote_amount_was_set":null}', 'Bots::DcaMultiAsset', '2026-11-16 07:43:24.595369', 1);
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
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (1, 1, 1, 1, '2026-11-16 07:43:24.577214', 1, '2026-11-16 07:43:24.577214', NULL, NULL, NULL);
INSERT INTO "exchange_assets" (rowid, "id", "asset_id", "available", "created_at", "exchange_id", "updated_at", "withdrawal_chains", "withdrawal_fee", "withdrawal_fee_updated_at") VALUES (2, 2, 2, 1, '2026-11-16 07:43:24.580493', 1, '2026-11-16 07:43:24.580493', NULL, NULL, NULL);
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
INSERT INTO "tickers" (rowid, "id", "ath", "ath_updated_at", "available", "base", "base_asset_id", "base_decimals", "created_at", "exchange_id", "maximum_base_size", "maximum_quote_size", "minimum_base_size", "minimum_quote_size", "price_decimals", "quote", "quote_asset_id", "quote_decimals", "ticker", "trading_enabled", "updated_at") VALUES (1, 1, NULL, NULL, 1, 'BTC', 1, 9, '2026-11-16 07:43:24.586554', 1, NULL, NULL, 2.7e-5, 1, 2, 'USD', 2, 2, 'BTC/USD', 1, '2026-11-16 07:43:24.586554');
CREATE TABLE "transactions" ("id" integer PRIMARY KEY AUTOINCREMENT NOT NULL, "amount" decimal, "amount_exec" decimal, "base" varchar, "base_asset_id" integer, "bot_id" bigint, "bot_interval" varchar DEFAULT '' NOT NULL, "bot_quote_amount" decimal DEFAULT 0.0 NOT NULL, "created_at" datetime NOT NULL, "error_messages" json DEFAULT '[]' NOT NULL, "exchange_id" bigint NOT NULL, "external_id" varchar, "external_status" integer, "order_type" integer, "price" decimal, "quote" varchar, "quote_amount" decimal, "quote_amount_exec" decimal, "quote_asset_id" integer, "side" integer, "status" integer, "transaction_type" varchar DEFAULT 'REGULAR' NOT NULL, "updated_at" datetime NOT NULL, "run_identity" json, CONSTRAINT "fk_rails_d4bc8411ec"
FOREIGN KEY ("bot_id")
  REFERENCES "bots" ("id")
, CONSTRAINT "fk_rails_e7352b1d47"
FOREIGN KEY ("exchange_id")
  REFERENCES "exchanges" ("id")
);
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
INSERT INTO "ar_internal_metadata" (rowid, "key", "value", "created_at", "updated_at") VALUES (1, 'environment', 'production', '2026-11-16 07:43:16.619', '2026-11-16 07:43:16.619');
DELETE FROM sqlite_sequence;
INSERT INTO sqlite_sequence (name, seq) VALUES ('account_balances', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('account_transactions', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('action_mcp_session_messages', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('action_mcp_session_subscriptions', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('api_keys', 1);
INSERT INTO sqlite_sequence (name, seq) VALUES ('bot_activity_logs', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('bot_index_assets', 1);
INSERT INTO sqlite_sequence (name, seq) VALUES ('bot_signals', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('bots', 1);
INSERT INTO sqlite_sequence (name, seq) VALUES ('connected_clients', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('exchange_assets', 2);
INSERT INTO sqlite_sequence (name, seq) VALUES ('fee_api_keys', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('fund_classifications', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('idempotency_keys', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('oauth_applications', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('rule_logs', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('rules', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('tickers', 1);
INSERT INTO sqlite_sequence (name, seq) VALUES ('transactions', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('wash_sale_locks', 0);
INSERT INTO sqlite_sequence (name, seq) VALUES ('users', 1);
INSERT INTO sqlite_sequence (name, seq) VALUES ('exchanges', 1);
INSERT INTO sqlite_sequence (name, seq) VALUES ('assets', 2);
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
