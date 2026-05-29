class AddWeightsToIndices < ActiveRecord::Migration[8.1]
  def change
    add_column :indices, :weights, :json, default: {}
  end
end
