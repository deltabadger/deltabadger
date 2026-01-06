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

ActiveRecord::Schema[8.1].define(version: 2026_01_06_162140) do
  # These are extensions that must be enabled in order to support this database
  enable_extension "pg_catalog.plpgsql"

  create_table "affiliates", force: :cascade do |t|
    t.boolean "active", default: true, null: false
    t.string "address"
    t.string "btc_address"
    t.string "code", null: false
    t.datetime "created_at", precision: nil, null: false
    t.decimal "discount_percent", precision: 3, scale: 2, null: false
    t.decimal "exported_btc_commission", precision: 16, scale: 8, default: "0.0", null: false
    t.string "name"
    t.string "new_btc_address"
    t.datetime "new_btc_address_send_at", precision: nil
    t.string "new_btc_address_token"
    t.string "old_code"
    t.decimal "paid_btc_commission", precision: 16, scale: 8, default: "0.0", null: false
    t.decimal "total_bonus_percent", precision: 3, scale: 2, null: false
    t.integer "type", null: false
    t.decimal "unexported_btc_commission", precision: 16, scale: 8, default: "0.0", null: false
    t.datetime "updated_at", precision: nil, null: false
    t.bigint "user_id"
    t.string "vat_number"
    t.string "visible_link"
    t.string "visible_link_scheme", default: "https", null: false
    t.string "visible_name"
    t.index ["code"], name: "index_affiliates_on_code", unique: true
    t.index ["new_btc_address_token"], name: "index_affiliates_on_new_btc_address_token", unique: true
    t.index ["user_id"], name: "index_affiliates_on_user_id", unique: true
  end

  create_table "ahoy_clicks", force: :cascade do |t|
    t.string "campaign"
    t.string "token"
    t.index ["campaign"], name: "index_ahoy_clicks_on_campaign"
  end

  create_table "ahoy_messages", force: :cascade do |t|
    t.string "campaign"
    t.string "mailer"
    t.datetime "sent_at", precision: nil
    t.text "subject"
    t.string "to"
    t.bigint "user_id"
    t.string "user_type"
    t.index ["campaign"], name: "index_ahoy_messages_on_campaign"
    t.index ["to"], name: "index_ahoy_messages_on_to"
    t.index ["user_type", "user_id"], name: "index_ahoy_messages_on_user_type_and_user_id"
  end

  create_table "ahoy_opens", force: :cascade do |t|
    t.string "campaign"
    t.string "token"
    t.index ["campaign"], name: "index_ahoy_opens_on_campaign"
  end

  create_table "api_keys", force: :cascade do |t|
    t.datetime "created_at", precision: nil, null: false
    t.string "encrypted_key"
    t.string "encrypted_key_iv"
    t.string "encrypted_passphrase"
    t.string "encrypted_passphrase_iv"
    t.string "encrypted_secret"
    t.string "encrypted_secret_iv"
    t.bigint "exchange_id", null: false
    t.boolean "german_trading_agreement"
    t.integer "key_type", default: 0, null: false
    t.integer "status", default: 0, null: false
    t.datetime "updated_at", precision: nil, null: false
    t.bigint "user_id", null: false
    t.index ["exchange_id"], name: "index_api_keys_on_exchange_id"
    t.index ["user_id"], name: "index_api_keys_on_user_id"
  end

  create_table "app_configs", force: :cascade do |t|
    t.datetime "created_at", null: false
    t.text "encrypted_value"
    t.string "encrypted_value_iv"
    t.string "key", null: false
    t.datetime "updated_at", null: false
    t.index ["key"], name: "index_app_configs_on_key", unique: true
  end

  create_table "articles", force: :cascade do |t|
    t.bigint "author_id"
    t.text "content", null: false
    t.datetime "created_at", null: false
    t.text "excerpt"
    t.string "locale", limit: 2, null: false
    t.text "paywall_hook"
    t.boolean "published", default: false, null: false
    t.datetime "published_at", precision: nil
    t.integer "reading_time_minutes"
    t.string "slug", null: false
    t.string "subtitle"
    t.string "telegram_url"
    t.string "thumbnail"
    t.string "title", null: false
    t.datetime "updated_at", null: false
    t.string "x_url"
    t.index ["author_id"], name: "index_articles_on_author_id"
    t.index ["locale"], name: "index_articles_on_locale"
    t.index ["published", "published_at"], name: "index_articles_on_published_and_published_at"
    t.index ["slug", "locale"], name: "index_articles_on_slug_and_locale", unique: true
  end

  create_table "assets", force: :cascade do |t|
    t.string "category"
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

  create_table "authors", force: :cascade do |t|
    t.string "avatar"
    t.text "bio"
    t.datetime "created_at", null: false
    t.string "name", null: false
    t.datetime "updated_at", null: false
    t.string "url"
    t.index ["name"], name: "index_authors_on_name"
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
    t.jsonb "settings", default: {}, null: false
    t.datetime "settings_changed_at", precision: nil
    t.datetime "started_at", precision: nil
    t.integer "status", default: 0, null: false
    t.string "stop_message_key"
    t.datetime "stopped_at", precision: nil
    t.jsonb "transient_data", default: {}, null: false
    t.string "type"
    t.datetime "updated_at", precision: nil, null: false
    t.bigint "user_id"
    t.index ["exchange_id"], name: "index_bots_on_exchange_id"
    t.index ["user_id"], name: "index_bots_on_user_id"
  end

  create_table "caffeinate_campaign_subscriptions", force: :cascade do |t|
    t.bigint "caffeinate_campaign_id", null: false
    t.datetime "created_at", null: false
    t.datetime "ended_at", precision: nil
    t.string "ended_reason"
    t.datetime "resubscribed_at", precision: nil
    t.integer "subscriber_id", null: false
    t.string "subscriber_type", null: false
    t.string "token", null: false
    t.string "unsubscribe_reason"
    t.datetime "unsubscribed_at", precision: nil
    t.datetime "updated_at", null: false
    t.integer "user_id"
    t.string "user_type"
    t.index ["caffeinate_campaign_id", "subscriber_id", "subscriber_type", "user_id", "user_type", "ended_at", "resubscribed_at", "unsubscribed_at"], name: "index_caffeinate_campaign_subscriptions"
    t.index ["caffeinate_campaign_id"], name: "caffeineate_campaign_subscriptions_on_campaign"
    t.index ["token"], name: "index_caffeinate_campaign_subscriptions_on_token", unique: true
  end

  create_table "caffeinate_campaigns", force: :cascade do |t|
    t.boolean "active", default: true, null: false
    t.datetime "created_at", null: false
    t.string "name", null: false
    t.string "slug", null: false
    t.datetime "updated_at", null: false
    t.index ["slug"], name: "index_caffeinate_campaigns_on_slug", unique: true
  end

  create_table "caffeinate_mailings", force: :cascade do |t|
    t.bigint "caffeinate_campaign_subscription_id", null: false
    t.datetime "created_at", null: false
    t.string "mailer_action", null: false
    t.string "mailer_class", null: false
    t.datetime "send_at", precision: nil, null: false
    t.datetime "sent_at", precision: nil
    t.datetime "skipped_at", precision: nil
    t.datetime "updated_at", null: false
    t.index ["caffeinate_campaign_subscription_id", "send_at", "sent_at", "skipped_at"], name: "index_caffeinate_mailings"
    t.index ["caffeinate_campaign_subscription_id"], name: "index_caffeinate_mailings_on_campaign_subscription"
  end

  create_table "cards", force: :cascade do |t|
    t.string "first_transaction_id"
    t.string "ip"
    t.string "token", null: false
    t.bigint "user_id", null: false
    t.index ["user_id"], name: "index_cards_on_user_id"
  end

  create_table "conversion_rates", force: :cascade do |t|
    t.datetime "created_at", precision: nil, null: false
    t.string "currency", null: false
    t.decimal "rate", null: false
    t.datetime "updated_at", precision: nil, null: false
    t.index ["currency"], name: "index_conversion_rates_on_currency", unique: true
  end

  create_table "countries", force: :cascade do |t|
    t.string "code"
    t.datetime "created_at", precision: nil, null: false
    t.integer "currency", default: 0, null: false
    t.boolean "eu_member", default: false, null: false
    t.string "name", null: false
    t.datetime "updated_at", precision: nil, null: false
    t.decimal "vat_rate", precision: 2, scale: 2, null: false
  end

  create_table "exchange_assets", force: :cascade do |t|
    t.bigint "asset_id", null: false
    t.boolean "available", default: true
    t.datetime "created_at", null: false
    t.bigint "exchange_id", null: false
    t.datetime "updated_at", null: false
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
    t.string "withdrawal_fee"
    t.index ["type"], name: "index_exchanges_on_type", unique: true
  end

  create_table "fee_api_keys", force: :cascade do |t|
    t.string "encrypted_key"
    t.string "encrypted_key_iv"
    t.string "encrypted_passphrase"
    t.string "encrypted_passphrase_iv"
    t.string "encrypted_secret"
    t.string "encrypted_secret_iv"
    t.bigint "exchange_id", null: false
    t.index ["exchange_id"], name: "index_fee_api_keys_on_exchange_id"
  end

  create_table "payments", force: :cascade do |t|
    t.date "birth_date"
    t.decimal "btc_commission", precision: 16, scale: 8, default: "0.0", null: false
    t.decimal "btc_paid", precision: 16, scale: 8, default: "0.0", null: false
    t.decimal "btc_total", precision: 16, scale: 8, default: "0.0", null: false
    t.decimal "commission", precision: 10, scale: 2, null: false
    t.boolean "commission_granted", default: false
    t.string "country", null: false
    t.datetime "created_at", precision: nil, null: false
    t.integer "currency", null: false
    t.boolean "discounted", null: false
    t.jsonb "external_statuses", default: []
    t.string "finger_print_id"
    t.string "first_name"
    t.boolean "gads_tracked", default: false
    t.string "last_name"
    t.datetime "paid_at", precision: nil
    t.string "payment_id"
    t.boolean "recurring", default: false, null: false
    t.integer "status", null: false
    t.bigint "subscription_plan_variant_id", null: false
    t.decimal "total", precision: 10, scale: 2, null: false
    t.string "type"
    t.datetime "updated_at", precision: nil, null: false
    t.string "url"
    t.bigint "user_id", null: false
    t.index ["currency"], name: "index_payments_on_currency"
    t.index ["status"], name: "index_payments_on_status"
    t.index ["subscription_plan_variant_id"], name: "index_payments_on_subscription_plan_variant_id"
    t.index ["type"], name: "index_payments_on_type"
    t.index ["user_id"], name: "index_payments_on_user_id"
  end

  create_table "portfolio_assets", force: :cascade do |t|
    t.decimal "allocation", precision: 5, scale: 4, default: "0.0", null: false
    t.string "api_id"
    t.string "category"
    t.string "color"
    t.string "country"
    t.datetime "created_at", null: false
    t.string "exchange"
    t.string "name"
    t.bigint "portfolio_id", null: false
    t.string "ticker"
    t.datetime "updated_at", null: false
    t.string "url"
    t.index ["portfolio_id"], name: "index_portfolio_assets_on_portfolio_id"
  end

  create_table "portfolios", force: :cascade do |t|
    t.date "backtest_start_date", default: "2020-01-01", null: false
    t.integer "benchmark", default: 0, null: false
    t.jsonb "compare_to", default: [], null: false
    t.datetime "created_at", null: false
    t.string "label"
    t.decimal "risk_free_rate", precision: 5, scale: 4, default: "0.0", null: false
    t.integer "risk_level", default: 2, null: false
    t.boolean "smart_allocation_on", default: false, null: false
    t.integer "strategy", default: 0, null: false
    t.datetime "updated_at", null: false
    t.bigint "user_id", null: false
    t.index ["user_id"], name: "index_portfolios_on_user_id"
  end

  create_table "setting_flags", force: :cascade do |t|
    t.string "name"
    t.boolean "value"
  end

  create_table "subscription_plan_variants", force: :cascade do |t|
    t.decimal "cost_eur", precision: 10, scale: 2
    t.decimal "cost_usd", precision: 10, scale: 2
    t.datetime "created_at", null: false
    t.integer "days"
    t.integer "subscription_plan_id", null: false
    t.datetime "updated_at", null: false
  end

  create_table "subscription_plans", force: :cascade do |t|
    t.datetime "created_at", precision: nil, null: false
    t.string "name"
    t.datetime "updated_at", precision: nil, null: false
  end

  create_table "subscriptions", force: :cascade do |t|
    t.boolean "auto_renew", default: false
    t.datetime "created_at", precision: nil, null: false
    t.datetime "ends_at", precision: nil
    t.string "eth_address"
    t.integer "nft_id"
    t.bigint "subscription_plan_variant_id"
    t.datetime "updated_at", precision: nil, null: false
    t.bigint "user_id"
    t.index ["nft_id"], name: "index_subscriptions_on_nft_id", unique: true, where: "(nft_id IS NOT NULL)"
    t.index ["subscription_plan_variant_id"], name: "index_subscriptions_on_subscription_plan_variant_id"
    t.index ["user_id"], name: "index_subscriptions_on_user_id"
  end

  create_table "surveys", force: :cascade do |t|
    t.jsonb "answers", default: {}, null: false
    t.datetime "created_at", null: false
    t.string "type", null: false
    t.datetime "updated_at", null: false
    t.bigint "user_id", null: false
    t.index ["user_id", "type"], name: "index_surveys_on_user_id_and_type", unique: true
    t.index ["user_id"], name: "index_surveys_on_user_id"
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
    t.string "called_bot_type"
    t.datetime "created_at", precision: nil, null: false
    t.jsonb "error_messages", default: [], null: false
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
    t.boolean "has_community_access", default: false
    t.datetime "last_otp_at", precision: nil
    t.string "locale"
    t.string "name"
    t.boolean "news_banner_dismissed", default: false
    t.string "oauth_provider"
    t.string "oauth_uid"
    t.integer "otp_module", default: 0
    t.string "otp_secret_key"
    t.integer "pending_plan_variant_id"
    t.string "pending_wire_transfer"
    t.boolean "referral_banner_dismissed", default: false
    t.bigint "referrer_id"
    t.datetime "remember_created_at", precision: nil
    t.datetime "reset_password_sent_at", precision: nil
    t.string "reset_password_token"
    t.boolean "show_smart_intervals_info", default: true, null: false
    t.boolean "subscribed_to_email_marketing", default: true
    t.boolean "terms_and_conditions"
    t.string "time_zone", default: "UTC", null: false
    t.string "unconfirmed_email"
    t.datetime "updated_at", precision: nil, null: false
    t.boolean "updates_agreement"
    t.boolean "welcome_banner_dismissed", default: false
    t.index ["confirmation_token"], name: "index_users_on_confirmation_token", unique: true
    t.index ["email"], name: "index_users_on_email", unique: true
    t.index ["reset_password_token"], name: "index_users_on_reset_password_token", unique: true
  end

  add_foreign_key "affiliates", "users"
  add_foreign_key "api_keys", "exchanges"
  add_foreign_key "api_keys", "users"
  add_foreign_key "articles", "authors"
  add_foreign_key "bots", "exchanges"
  add_foreign_key "bots", "users"
  add_foreign_key "caffeinate_campaign_subscriptions", "caffeinate_campaigns"
  add_foreign_key "caffeinate_mailings", "caffeinate_campaign_subscriptions"
  add_foreign_key "cards", "users"
  add_foreign_key "exchange_assets", "assets"
  add_foreign_key "exchange_assets", "exchanges"
  add_foreign_key "fee_api_keys", "exchanges"
  add_foreign_key "payments", "subscription_plan_variants"
  add_foreign_key "payments", "users"
  add_foreign_key "portfolio_assets", "portfolios"
  add_foreign_key "portfolios", "users"
  add_foreign_key "subscription_plan_variants", "subscription_plans"
  add_foreign_key "subscriptions", "subscription_plan_variants"
  add_foreign_key "subscriptions", "users"
  add_foreign_key "surveys", "users"
  add_foreign_key "tickers", "assets", column: "base_asset_id"
  add_foreign_key "tickers", "assets", column: "quote_asset_id"
  add_foreign_key "tickers", "exchanges"
  add_foreign_key "transactions", "bots"
  add_foreign_key "transactions", "exchanges"
  add_foreign_key "users", "affiliates", column: "referrer_id"
  add_foreign_key "users", "subscription_plan_variants", column: "pending_plan_variant_id"
end
