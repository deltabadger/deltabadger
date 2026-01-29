class RemoveCoinsCountFromIndices < ActiveRecord::Migration[8.1]
  def change
    remove_column :indices, :coins_count, :integer
  end
end
