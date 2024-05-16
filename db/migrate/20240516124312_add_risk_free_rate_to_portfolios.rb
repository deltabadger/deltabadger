class AddRiskFreeRateToPortfolios < ActiveRecord::Migration[6.0]
  def change
    add_column :portfolios, :risk_free_rate, :float, default: 0.0, null: false
  end
end
