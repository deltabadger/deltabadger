class AddAvailableExchangesToIndices < ActiveRecord::Migration[8.1]
  def change
    add_column :indices, :available_exchanges, :json, default: {}
  end
end
