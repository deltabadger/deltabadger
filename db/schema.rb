# This file is auto-generated from the current state of the database. Instead
# of editing this file, please use the migrations feature of Active Record to
# incrementally modify your database, and then regenerate this schema definition.
#
# This file is the source Rails uses to define your schema when running `bin/rails
# db:schema:load`. When creating a new database, `bin/rails db:schema:load` tends to
# be faster and is potentially less error prone than running all of your
# migrations from scratch. Old migrations may fail to apply correctly if those
# migrations use external dependencies or application code.
#
# It's strongly recommended that you check this file into your version control system.

ActiveRecord::Schema[8.1].define(version: 2026_02_19_130000) do
  create_table "api_keys", force: :cascade do |t|
    t.datetime "created_at", precision: nil, null: false
    t.bigint "exchange_id", null: false
    t.boolean "german_trading_agreement"
    t.string "key"
    t.integer "key_type", default: 0, null: false
    t.string "passphrase"
    t.string "secret"
    t.integer "status", default: 0, null: false
    t.datetime "updated_at", precision: nil, null: false
    t.bigint "user_id", null: false
    t.index ["exchange_id"], name: "index_api_keys_on_exchange_id"
    t.index ["user_id"], name: "index_api_keys_on_user_id"
  end

  create_table "app_configs", force: :cascade do |t|
    t.datetime "created_at", null: false
    t.string "key", null: false
    t.datetime "updated_at", null: false
    t.text "value"
    t.index ["key"], name: "index_app_configs_on_key", unique: true
  end

  create_table "assets", force: :cascade do |t|
    t.string "category"
    t.decimal "circulating_supply", precision: 30, scale: 8
    t.string "color"
    t.string "country"
    t.string "country_exchange"
    t.datetime "created_at", null: false
    t.string "external_id", null: false
    t.string "image_url"
    t.string "isin"
    t.bigint "market_cap"
    t.integer "market_cap_rank"
    t.string "name"
    t.string "symbol"
    t.datetime "updated_at", null: false
    t.string "url"
    t.index ["external_id"], name: "index_assets_on_external_id", unique: true
    t.index ["isin"], name: "index_assets_on_isin"
    t.index ["name"], name: "index_assets_on_name"
    t.index ["symbol"], name: "index_assets_on_symbol"
  end

  create_table "bot_index_assets", force: :cascade do |t|
    t.integer "asset_id", null: false
    t.integer "bot_id", null: false
    t.datetime "created_at", null: false
    t.decimal "current_allocation", precision: 10, scale: 6
    t.datetime "entered_at"
    t.datetime "exited_at"
    t.boolean "in_index", default: true
    t.decimal "target_allocation", precision: 10, scale: 6
    t.integer "ticker_id", null: false
    t.datetime "updated_at", null: false
    t.index ["asset_id"], name: "index_bot_index_assets_on_asset_id"
    t.index ["bot_id", "asset_id"], name: "index_bot_index_assets_on_bot_id_and_asset_id", unique: true
    t.index ["bot_id"], name: "index_bot_index_assets_on_bot_id"
    t.index ["ticker_id"], name: "index_bot_index_assets_on_ticker_id"
  end

  create_table "bots", force: :cascade do |t|
    t.decimal "account_balance", default: "0.0"
    t.datetime "created_at", precision: nil, null: false
    t.integer "current_delay", default: 0, null: false
    t.integer "delay", default: 0, null: false
    t.bigint "exchange_id"
    t.integer "fetch_restarts", default: 0, null: false
    t.string "label"
    t.datetime "last_end_of_funds_notification", precision: nil
    t.integer "restarts", default: 0, null: false
    t.json "settings", default: {}, null: false
    t.datetime "settings_changed_at", precision: nil
    t.datetime "started_at", precision: nil
    t.integer "status", default: 0, null: false
    t.string "stop_message_key"
    t.datetime "stopped_at", precision: nil
    t.json "transient_data", default: {}, null: false
    t.string "type"
    t.datetime "updated_at", precision: nil, null: false
    t.bigint "user_id"
    t.index ["exchange_id"], name: "index_bots_on_exchange_id"
    t.index ["user_id"], name: "index_bots_on_user_id"
  end

  create_table "exchange_assets", force: :cascade do |t|
    t.bigint "asset_id", null: false
    t.boolean "available", default: true
    t.datetime "created_at", null: false
    t.bigint "exchange_id", null: false
    t.datetime "updated_at", null: false
    t.json "withdrawal_chains"
    t.string "withdrawal_fee"
    t.datetime "withdrawal_fee_updated_at"
    t.index ["asset_id", "exchange_id"], name: "index_exchange_assets_on_asset_id_and_exchange_id", unique: true
    t.index ["asset_id"], name: "index_exchange_assets_on_asset_id"
    t.index ["exchange_id"], name: "index_exchange_assets_on_exchange_id"
  end

  create_table "exchanges", force: :cascade do |t|
    t.boolean "available", default: true
    t.datetime "created_at", precision: nil, null: false
    t.string "maker_fee"
    t.string "name"
    t.string "taker_fee"
    t.string "type"
    t.datetime "updated_at", precision: nil, null: false
    t.index ["type"], name: "index_exchanges_on_type", unique: true
  end

  create_table "fee_api_keys", force: :cascade do |t|
    t.bigint "exchange_id", null: false
    t.string "key"
    t.string "passphrase"
    t.string "secret"
    t.index ["exchange_id"], name: "index_fee_api_keys_on_exchange_id"
  end

  create_table "indices", force: :cascade do |t|
    t.json "available_exchanges", default: {}
    t.datetime "created_at", null: false
    t.text "description"
    t.string "external_id"
    t.decimal "market_cap"
    t.string "name"
    t.string "source"
    t.json "top_coins"
    t.json "top_coins_by_exchange", default: {}
    t.datetime "updated_at", null: false
    t.integer "weight", default: 0, null: false
    t.index ["external_id", "source"], name: "index_indices_on_external_id_and_source", unique: true
    t.index ["weight"], name: "index_indices_on_weight"
  end

  create_table "rule_logs", force: :cascade do |t|
    t.datetime "created_at", null: false
    t.json "details", default: {}, null: false
    t.string "message"
    t.integer "rule_id", null: false
    t.integer "status", default: 0, null: false
    t.index ["rule_id", "created_at"], name: "index_rule_logs_on_rule_id_and_created_at"
    t.index ["rule_id"], name: "index_rule_logs_on_rule_id"
  end

  create_table "rules", force: :cascade do |t|
    t.string "address"
    t.integer "asset_id"
    t.datetime "created_at", null: false
    t.integer "exchange_id"
    t.json "settings", default: {}, null: false
    t.datetime "settings_changed_at"
    t.integer "status", default: 0, null: false
    t.string "type", null: false
    t.datetime "updated_at", null: false
    t.integer "user_id", null: false
    t.index ["asset_id"], name: "index_rules_on_asset_id"
    t.index ["exchange_id"], name: "index_rules_on_exchange_id"
    t.index ["user_id", "type", "exchange_id", "asset_id"], name: "idx_rules_user_type_exchange_asset", unique: true
    t.index ["user_id"], name: "index_rules_on_user_id"
  end

  create_table "setting_flags", force: :cascade do |t|
    t.string "name"
    t.boolean "value"
  end

  create_table "tickers", force: :cascade do |t|
    t.decimal "ath"
    t.datetime "ath_updated_at", precision: nil
    t.boolean "available", default: true
    t.string "base", null: false
    t.bigint "base_asset_id", null: false
    t.integer "base_decimals", null: false
    t.datetime "created_at", null: false
    t.bigint "exchange_id", null: false
    t.decimal "maximum_base_size"
    t.decimal "maximum_quote_size"
    t.decimal "minimum_base_size", null: false
    t.decimal "minimum_quote_size", null: false
    t.integer "price_decimals", null: false
    t.string "quote", null: false
    t.bigint "quote_asset_id", null: false
    t.integer "quote_decimals", null: false
    t.string "ticker", null: false
    t.datetime "updated_at", null: false
    t.index ["base_asset_id"], name: "index_tickers_on_base_asset_id"
    t.index ["exchange_id", "base", "quote"], name: "index_exchange_tickers_on_unique_base_and_quote", unique: true
    t.index ["exchange_id", "base_asset_id", "quote_asset_id"], name: "index_exchange_tickers_on_unique_base_asset_and_quote_asset", unique: true
    t.index ["exchange_id", "ticker"], name: "index_exchange_tickers_on_unique_ticker", unique: true
    t.index ["exchange_id"], name: "index_tickers_on_exchange_id"
    t.index ["quote_asset_id"], name: "index_tickers_on_quote_asset_id"
  end

  create_table "transactions", force: :cascade do |t|
    t.decimal "amount"
    t.decimal "amount_exec"
    t.string "base"
    t.bigint "bot_id"
    t.string "bot_interval", default: "", null: false
    t.decimal "bot_quote_amount", default: "0.0", null: false
    t.datetime "created_at", precision: nil, null: false
    t.json "error_messages", default: [], null: false
    t.bigint "exchange_id", null: false
    t.string "external_id"
    t.integer "external_status"
    t.integer "order_type"
    t.decimal "price"
    t.string "quote"
    t.decimal "quote_amount"
    t.decimal "quote_amount_exec"
    t.integer "side"
    t.integer "status"
    t.string "transaction_type", default: "REGULAR", null: false
    t.datetime "updated_at", precision: nil, null: false
    t.index ["bot_id", "created_at"], name: "index_transactions_on_bot_id_and_created_at"
    t.index ["bot_id", "status", "created_at"], name: "index_transactions_on_bot_id_and_status_and_created_at"
    t.index ["bot_id", "transaction_type", "created_at"], name: "index_bot_type_created_at"
    t.index ["bot_id"], name: "index_transactions_on_bot_id"
    t.index ["created_at"], name: "index_transactions_on_created_at"
    t.index ["exchange_id"], name: "index_transactions_on_exchange_id"
    t.index ["external_id"], name: "index_transactions_on_external_id", unique: true
  end

  create_table "users", force: :cascade do |t|
    t.boolean "admin", default: false, null: false
    t.datetime "confirmation_sent_at", precision: nil
    t.string "confirmation_token"
    t.datetime "confirmed_at", precision: nil
    t.datetime "created_at", precision: nil, null: false
    t.string "email", default: "", null: false
    t.string "encrypted_password", default: "", null: false
    t.datetime "last_otp_at", precision: nil
    t.string "locale"
    t.string "name"
    t.integer "otp_module", default: 0
    t.string "otp_secret_key"
    t.datetime "remember_created_at", precision: nil
    t.datetime "reset_password_sent_at", precision: nil
    t.string "reset_password_token"
    t.boolean "setup_completed", default: false, null: false
    t.boolean "show_smart_intervals_info", default: true, null: false
    t.boolean "subscribed_to_email_marketing", default: true
    t.string "time_zone", default: "UTC", null: false
    t.string "unconfirmed_email"
    t.datetime "updated_at", precision: nil, null: false
    t.index ["confirmation_token"], name: "index_users_on_confirmation_token", unique: true
    t.index ["email"], name: "index_users_on_email", unique: true
    t.index ["reset_password_token"], name: "index_users_on_reset_password_token", unique: true
  end

  add_foreign_key "api_keys", "exchanges"
  add_foreign_key "api_keys", "users"
  add_foreign_key "bot_index_assets", "assets"
  add_foreign_key "bot_index_assets", "bots"
  add_foreign_key "bot_index_assets", "tickers"
  add_foreign_key "bots", "exchanges"
  add_foreign_key "bots", "users"
  add_foreign_key "exchange_assets", "assets"
  add_foreign_key "exchange_assets", "exchanges"
  add_foreign_key "fee_api_keys", "exchanges"
  add_foreign_key "rule_logs", "rules"
  add_foreign_key "rules", "assets"
  add_foreign_key "rules", "exchanges"
  add_foreign_key "rules", "users"
  add_foreign_key "tickers", "assets", column: "base_asset_id"
  add_foreign_key "tickers", "assets", column: "quote_asset_id"
  add_foreign_key "tickers", "exchanges"
  add_foreign_key "transactions", "bots"
  add_foreign_key "transactions", "exchanges"
end
