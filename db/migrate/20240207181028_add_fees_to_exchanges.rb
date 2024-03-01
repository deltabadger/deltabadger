class AddFeesToExchanges < ActiveRecord::Migration[6.0]
  def change
    add_column :exchanges, :taker_fee, :string
    add_column :exchanges, :withdrawal_fee, :string
  end
end
