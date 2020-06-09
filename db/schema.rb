# This file is auto-generated from the current state of the database. Instead
# of editing this file, please use the migrations feature of Active Record to
# incrementally modify your database, and then regenerate this schema definition.
#
# Note that this schema.rb definition is the authoritative source for your
# database schema. If you need to create the application database on another
# system, you should be using db:schema:load, not running all the migrations
# from scratch. The latter is a flawed and unsustainable approach (the more migrations
# you'll amass, the slower it'll run and the greater likelihood for issues).
#
# It's strongly recommended that you check this file into your version control system.

ActiveRecord::Schema.define(version: 2020_06_19_114111) do

  # These are extensions that must be enabled in order to support this database
  enable_extension "plpgsql"

  create_table "affiliates", force: :cascade do |t|
    t.bigint "user_id"
    t.string "first_name", null: false
    t.string "last_name", null: false
    t.date "birth_date"
    t.boolean "eu"
    t.string "btc_address", null: false
    t.string "code", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
    t.index ["code"], name: "index_affiliates_on_code", unique: true
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
    t.index ["exchange_id"], name: "index_bots_on_exchange_id"
    t.index ["user_id"], name: "index_bots_on_user_id"
  end

  create_table "exchanges", force: :cascade do |t|
    t.string "name"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
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
    t.boolean "eu"
    t.datetime "paid_at"
    t.string "external_statuses", default: "", null: false
    t.decimal "crypto_total", precision: 20, scale: 10, default: "0.0", null: false
    t.decimal "crypto_paid", precision: 20, scale: 10, default: "0.0", null: false
    t.index ["user_id"], name: "index_payments_on_user_id"
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
  end

  create_table "subscriptions", force: :cascade do |t|
    t.bigint "subscription_plan_id"
    t.bigint "user_id"
    t.datetime "end_time"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
    t.decimal "credits"
    t.boolean "limit_almost_reached_sent", default: false
    t.index ["subscription_plan_id"], name: "index_subscriptions_on_subscription_plan_id"
    t.index ["user_id"], name: "index_subscriptions_on_user_id"
  end

  create_table "transactions", force: :cascade do |t|
    t.bigint "bot_id"
    t.uuid "offer_id"
    t.decimal "rate"
    t.decimal "amount"
    t.string "market"
    t.integer "status"
    t.integer "currency"
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
    t.string "error_messages", default: "[]"
    t.index ["bot_id"], name: "index_transactions_on_bot_id"
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
    t.index ["confirmation_token"], name: "index_users_on_confirmation_token", unique: true
    t.index ["email"], name: "index_users_on_email", unique: true
    t.index ["reset_password_token"], name: "index_users_on_reset_password_token", unique: true
  end

  add_foreign_key "affiliates", "users"
  add_foreign_key "api_keys", "exchanges"
  add_foreign_key "api_keys", "users"
  add_foreign_key "bots", "exchanges"
  add_foreign_key "bots", "users"
  add_foreign_key "subscriptions", "subscription_plans"
  add_foreign_key "subscriptions", "users"
  add_foreign_key "transactions", "bots"
end
