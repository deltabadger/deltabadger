class RemoveMaxProfitFromAffiliates < ActiveRecord::Migration[6.0]
  def change
    remove_column :affiliates, :max_profit, :decimal, default: 50.0, null: false, precision: 12, scale: 2
  end
end
