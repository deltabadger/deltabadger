class AddPricedAtToAccountBalances < ActiveRecord::Migration[8.1]
  def change
    add_column :account_balances, :priced_at, :datetime
  end
end
