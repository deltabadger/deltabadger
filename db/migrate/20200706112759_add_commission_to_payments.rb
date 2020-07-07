class AddCommissionToPayments < ActiveRecord::Migration[5.2]
  def change
    add_column :payments, :commission, :decimal, null: false, default: 0
    add_column :payments, :crypto_commission, :decimal, precision: 20, scale: 10, null: false, default: 0
  end
end
