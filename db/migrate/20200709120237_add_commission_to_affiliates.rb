class AddCommissionToAffiliates < ActiveRecord::Migration[5.2]
  def change
    add_column :affiliates, :unexported_crypto_commission, :decimal, precision: 20, scale: 10, null: false, default: 0
    add_column :affiliates, :exported_crypto_commission, :decimal, precision: 20, scale: 10, null: false, default: 0
    add_column :affiliates, :paid_crypto_commission, :decimal, precision: 20, scale: 10, null: false, default: 0
  end
end
