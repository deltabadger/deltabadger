class RemoveCurrentReferrerProfitFromUsers < ActiveRecord::Migration[6.0]
  def change
    remove_column :users, :current_referrer_profit, :decimal, default: 0.0, null: false
  end
end
