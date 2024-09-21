class AddCompareToPortfolios < ActiveRecord::Migration[6.0]
  def change
    add_column :portfolios, :compare_to, :json, default: []
  end
end
