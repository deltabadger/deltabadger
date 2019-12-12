class ConvertCreditsFromIntToDecimal < ActiveRecord::Migration[5.2]
  def change
    change_column :subscriptions, :credits, :decimal
  end
end
