class AddColorToPortfolios < ActiveRecord::Migration[6.0]
  def change
    add_column :portfolios, :color, :string
  end
end
