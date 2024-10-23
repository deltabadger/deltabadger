class RenameCryptoToBitcoin < ActiveRecord::Migration[6.0]
  def change
    rename_column :payments, :crypto_total, :btc_total
    change_column :payments, :btc_total, :decimal, precision: 16, scale: 8

    rename_column :payments, :crypto_paid, :btc_paid
    change_column :payments, :btc_paid, :decimal, precision: 16, scale: 8

    rename_column :payments, :crypto_commission, :btc_commission
    change_column :payments, :btc_commission, :decimal, precision: 16, scale: 8

    rename_column :affiliates, :unexported_crypto_commission, :unexported_btc_commission
    change_column :affiliates, :unexported_btc_commission, :decimal, precision: 16, scale: 8

    rename_column :affiliates, :exported_crypto_commission, :exported_btc_commission
    change_column :affiliates, :exported_btc_commission, :decimal, precision: 16, scale: 8

    rename_column :affiliates, :paid_crypto_commission, :paid_btc_commission
    change_column :affiliates, :paid_btc_commission, :decimal, precision: 16, scale: 8
  end
end
