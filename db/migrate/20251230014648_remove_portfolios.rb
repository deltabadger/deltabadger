class RemovePortfolios < ActiveRecord::Migration[6.0]
  def change
    drop_table :portfolio_assets
    drop_table :portfolios
  end
end
