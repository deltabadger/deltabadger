class RenameAssetsToPortfolioAssets < ActiveRecord::Migration[6.0]
  def change
    rename_table :assets, :portfolio_assets
  end
end
