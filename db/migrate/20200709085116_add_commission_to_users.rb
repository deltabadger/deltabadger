class AddCommissionToUsers < ActiveRecord::Migration[5.2]
  def change
    add_column :users, :current_referrer_profit, :decimal, null: false, default: 0
  end
end
