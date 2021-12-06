class AddAccountBalanceToBots < ActiveRecord::Migration[5.2]
  def change
    add_column :bots, :account_balance, :decimal, default: 0.0
  end
end
