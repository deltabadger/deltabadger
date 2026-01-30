class AddWeightToIndices < ActiveRecord::Migration[8.1]
  def change
    add_column :indices, :weight, :integer, default: 0, null: false
    add_index :indices, :weight
  end
end
