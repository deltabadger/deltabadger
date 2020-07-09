class AddCommissionToUsers < ActiveRecord::Migration[5.2]
  def change
    add_column :users, :current_referrer_profit, :decimal, null: false, default: 0
    add_column :users, :unexported_crypto_commission, :decimal, precision: 20, scale: 10, null: false, default: 0
    add_column :users, :exported_crypto_commission, :decimal, precision: 20, scale: 10, null: false, default: 0
    add_column :users, :paid_crypto_commission, :decimal, precision: 20, scale: 10, null: false, default: 0
  end
end
