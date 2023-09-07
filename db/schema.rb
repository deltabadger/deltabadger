# This file is auto-generated from the current state of the database. Instead
# of editing this file, please use the migrations feature of Active Record to
# incrementally modify your database, and then regenerate this schema definition.
#
# This file is the source Rails uses to define your schema when running `rails
# db:schema:load`. When creating a new database, `rails db:schema:load` tends to
# be faster and is potentially less error prone than running all of your
# migrations from scratch. Old migrations may fail to apply correctly if those
# migrations use external dependencies or application code.
#
# It's strongly recommended that you check this file into your version control system.

ActiveRecord::Schema.define(version: 2023_09_07_145250) do

  # These are extensions that must be enabled in order to support this database
  enable_extension "plpgsql"

  create_table "affiliates", force: :cascade do |t|
    t.bigint "user_id"
    t.integer "type", null: false
    t.string "name"
    t.string "address"
    t.string "vat_number"
    t.string "btc_address"
    t.string "code", null: false
    t.string "visible_name"
    t.string "visible_link"
    t.decimal "max_profit", precision: 12, scale: 2, null: false
    t.decimal "discount_percent", precision: 3, scale: 2, null: false
    t.decimal "total_bonus_percent", precision: 3, scale: 2, null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
    t.string "new_btc_address"
    t.string "new_btc_address_token"
    t.datetime "new_btc_address_send_at"
    t.boolean "active", default: true, null: false
    t.decimal "unexported_crypto_commission", precision: 20, scale: 10, default: "0.0", null: false
    t.decimal "exported_crypto_commission", precision: 20, scale: 10, default: "0.0", null: false
    t.decimal "paid_crypto_commission", precision: 20, scale: 10, default: "0.0", null: false
    t.string "visible_link_scheme", default: "https", null: false
    t.string "old_code"
    t.index ["code"], name: "index_affiliates_on_code", unique: true
    t.index ["new_btc_address_token"], name: "index_affiliates_on_new_btc_address_token", unique: true
    t.index ["user_id"], name: "index_affiliates_on_user_id", unique: true
  end

  create_table "api_keys", force: :cascade do |t|
    t.bigint "exchange_id", null: false
    t.bigint "user_id", null: false
    t.string "encrypted_key"
    t.string "encrypted_key_iv"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
    t.string "encrypted_secret"
    t.string "encrypted_secret_iv"
    t.boolean "german_trading_agreement"
    t.string "encrypted_passphrase"
    t.string "encrypted_passphrase_iv"
    t.integer "status", default: 0, null: false
    t.integer "key_type", default: 0, null: false
    t.index ["exchange_id"], name: "index_api_keys_on_exchange_id"
    t.index ["user_id"], name: "index_api_keys_on_user_id"
  end

  create_table "bots", force: :cascade do |t|
    t.bigint "exchange_id"
    t.integer "status", default: 0, null: false
    t.bigint "user_id"
    t.jsonb "settings", default: "", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
    t.integer "bot_type"
    t.integer "restarts", default: 0, null: false
    t.integer "delay", default: 0, null: false
    t.integer "current_delay", default: 0, null: false
    t.datetime "settings_changed_at"
    t.integer "fetch_restarts", default: 0, null: false
    t.decimal "account_balance", default: "0.0"
    t.datetime "last_end_of_funds_notification"
    t.index ["exchange_id"], name: "index_bots_on_exchange_id"
    t.index ["user_id"], name: "index_bots_on_user_id"
  end

  create_table "conversion_rates", force: :cascade do |t|
    t.string "currency", null: false
    t.decimal "rate", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
    t.index ["currency"], name: "index_conversion_rates_on_currency", unique: true
  end

  create_table "daily_transaction_aggregates", force: :cascade do |t|
    t.bigint "bot_id"
    t.string "offer_id"
    t.decimal "rate"
    t.decimal "amount"
    t.string "market"
    t.integer "status"
    t.integer "currency"
    t.string "error_messages", default: "[]"
    t.decimal "bot_price", precision: 20, scale: 10, default: "0.0", null: false
    t.string "bot_interval", default: "", null: false
    t.string "transaction_type", default: "REGULAR", null: false
    t.string "called_bot_type"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
    t.decimal "total_amount", precision: 20, scale: 10, default: "0.0", null: false
    t.decimal "total_value", precision: 20, scale: 10, default: "0.0", null: false
    t.decimal "total_invested", precision: 20, scale: 10, default: "0.0", null: false
    t.index ["bot_id", "created_at"], name: "index_daily_transaction_aggregates_on_bot_id_and_created_at"
    t.index ["bot_id", "status", "created_at"], name: "dailies_index_status_created_at"
    t.index ["bot_id", "transaction_type", "created_at"], name: "dailies_index_bot_type_created_at"
    t.index ["bot_id"], name: "index_daily_transaction_aggregates_on_bot_id"
    t.index ["created_at"], name: "index_daily_transaction_aggregates_on_created_at"
  end

  create_table "exchanges", force: :cascade do |t|
    t.string "name"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end

  create_table "fee_api_keys", force: :cascade do |t|
    t.bigint "exchange_id", null: false
    t.string "encrypted_key"
    t.string "encrypted_key_iv"
    t.string "encrypted_secret"
    t.string "encrypted_secret_iv"
    t.string "encrypted_passphrase"
    t.string "encrypted_passphrase_iv"
    t.index ["exchange_id"], name: "index_fee_api_keys_on_exchange_id"
  end

  create_table "payments", force: :cascade do |t|
    t.string "payment_id"
    t.integer "status"
    t.decimal "total"
    t.integer "currency"
    t.bigint "user_id"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
    t.string "first_name"
    t.string "last_name"
    t.date "birth_date"
    t.datetime "paid_at"
    t.string "external_statuses", default: "", null: false
    t.decimal "crypto_total", precision: 20, scale: 10, default: "0.0", null: false
    t.decimal "crypto_paid", precision: 20, scale: 10, default: "0.0", null: false
    t.decimal "commission", default: "0.0", null: false
    t.decimal "crypto_commission", precision: 20, scale: 10, default: "0.0", null: false
    t.boolean "discounted", default: false, null: false
    t.bigint "subscription_plan_id", null: false
    t.string "country", null: false
    t.integer "payment_type", default: 0, null: false
    t.index ["subscription_plan_id"], name: "index_payments_on_subscription_plan_id"
    t.index ["user_id"], name: "index_payments_on_user_id"
  end

  create_table "setting_flags", force: :cascade do |t|
    t.string "name"
    t.boolean "value"
  end

  create_table "subscribers", force: :cascade do |t|
    t.string "email", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
    t.index ["email"], name: "index_subscribers_on_email", unique: true
  end

  create_table "subscription_plans", force: :cascade do |t|
    t.string "name"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
    t.integer "years", default: 1, null: false
    t.integer "credits", default: 1200, null: false
    t.boolean "unlimited", default: false, null: false
    t.decimal "cost_eu", default: "0.0", null: false
    t.decimal "cost_other", default: "0.0", null: false
  end

  create_table "subscriptions", force: :cascade do |t|
    t.bigint "subscription_plan_id"
    t.bigint "user_id"
    t.datetime "end_time"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
    t.decimal "credits"
    t.boolean "limit_almost_reached_sent", default: false
    t.datetime "first_month_ending_sent_at"
    t.integer "sequence_number"
    t.index ["subscription_plan_id"], name: "index_subscriptions_on_subscription_plan_id"
    t.index ["user_id"], name: "index_subscriptions_on_user_id"
  end

  create_table "transactions", force: :cascade do |t|
    t.bigint "bot_id"
    t.string "offer_id"
    t.decimal "rate"
    t.decimal "amount"
    t.string "market"
    t.integer "status"
    t.integer "currency"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
    t.string "error_messages", default: "[]"
    t.decimal "bot_price", precision: 20, scale: 10, default: "0.0", null: false
    t.string "bot_interval", default: "", null: false
    t.string "transaction_type", default: "REGULAR", null: false
    t.string "called_bot_type"
    t.index ["bot_id", "created_at"], name: "index_transactions_on_bot_id_and_created_at"
    t.index ["bot_id", "status", "created_at"], name: "index_transactions_on_bot_id_and_status_and_created_at"
    t.index ["bot_id", "transaction_type", "created_at"], name: "index_bot_type_created_at"
    t.index ["bot_id"], name: "index_transactions_on_bot_id"
    t.index ["created_at"], name: "index_transactions_on_created_at"
  end

  create_table "users", force: :cascade do |t|
    t.string "email", default: "", null: false
    t.string "encrypted_password", default: "", null: false
    t.string "reset_password_token"
    t.datetime "reset_password_sent_at"
    t.datetime "remember_created_at"
    t.string "confirmation_token"
    t.datetime "confirmed_at"
    t.datetime "confirmation_sent_at"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
    t.string "unconfirmed_email"
    t.boolean "admin", default: false, null: false
    t.boolean "terms_and_conditions"
    t.boolean "updates_agreement"
    t.boolean "welcome_banner_showed", default: false
    t.bigint "referrer_id"
    t.decimal "current_referrer_profit", default: "0.0", null: false
    t.boolean "show_smart_intervals_info", default: true, null: false
    t.string "pending_wire_transfer"
    t.integer "pending_plan_id"
    t.string "otp_secret_key"
    t.integer "otp_module", default: 0
    t.boolean "referral_banner_showed", default: false
    t.datetime "last_otp_at"
    t.string "name"
    t.index ["confirmation_token"], name: "index_users_on_confirmation_token", unique: true
    t.index ["email"], name: "index_users_on_email", unique: true
    t.index ["reset_password_token"], name: "index_users_on_reset_password_token", unique: true
  end

  create_table "vat_rates", force: :cascade do |t|
    t.string "country", null: false
    t.decimal "vat", precision: 2, scale: 2, null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end

  add_foreign_key "affiliates", "users"
  add_foreign_key "api_keys", "exchanges"
  add_foreign_key "api_keys", "users"
  add_foreign_key "bots", "exchanges"
  add_foreign_key "bots", "users"
  add_foreign_key "daily_transaction_aggregates", "bots"
  add_foreign_key "fee_api_keys", "exchanges"
  add_foreign_key "payments", "subscription_plans"
  add_foreign_key "subscriptions", "subscription_plans"
  add_foreign_key "subscriptions", "users"
  add_foreign_key "transactions", "bots"
  add_foreign_key "users", "affiliates", column: "referrer_id"

  create_view "bots_total_amounts", materialized: true, sql_definition: <<-SQL
      SELECT transactions.bot_id,
      sum((transactions.amount * transactions.rate)) AS total_cost,
      sum(transactions.amount) AS total_amount,
      bots.exchange_id,
      bots.settings,
      bots.created_at
     FROM (bots
       JOIN transactions ON ((transactions.bot_id = bots.id)))
    WHERE ((bots.settings ->> 'type'::text) = 'buy'::text)
    GROUP BY transactions.bot_id, bots.exchange_id, bots.settings, bots.created_at;
  SQL
  add_index "bots_total_amounts", ["bot_id"], name: "index_bots_total_amounts_on_bot_id", unique: true

end
