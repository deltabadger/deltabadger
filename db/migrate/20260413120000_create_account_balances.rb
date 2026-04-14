class CreateAccountBalances < ActiveRecord::Migration[8.1]
  def change
    create_table :account_balances do |t|
      t.references :user, null: false, foreign_key: true
      t.references :exchange, null: false, foreign_key: true
      t.references :asset, null: false, foreign_key: true
      t.decimal :free, precision: 32, scale: 16, default: 0, null: false
      t.decimal :locked, precision: 32, scale: 16, default: 0, null: false
      t.decimal :usd_price, precision: 20, scale: 8
      t.decimal :usd_value, precision: 20, scale: 8
      t.datetime :synced_at, null: false

      t.timestamps
    end

    add_index :account_balances, [:user_id, :exchange_id, :asset_id], unique: true, name: 'idx_account_balances_user_exchange_asset'
  end
end
