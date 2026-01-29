class AddTopCoinsByExchangeToIndices < ActiveRecord::Migration[8.1]
  def change
    add_column :indices, :top_coins_by_exchange, :json, default: {}
  end
end
