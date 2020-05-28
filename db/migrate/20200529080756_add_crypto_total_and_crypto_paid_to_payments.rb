class AddCryptoTotalAndCryptoPaidToPayments < ActiveRecord::Migration[5.2]
  def change
    add_column :payments, :crypto_total, :decimal, precision: 20, scale: 10
    add_column :payments, :crypto_paid, :decimal, precision: 20, scale: 10
  end
end
