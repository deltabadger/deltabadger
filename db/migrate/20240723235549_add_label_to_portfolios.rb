class AddLabelToPortfolios < ActiveRecord::Migration[6.0]
  def change
    add_column :portfolios, :label, :string
  end
end
