class CreateExchangeTickers < ActiveRecord::Migration[6.0]
  def change
    create_table :exchange_tickers do |t|
      t.references :exchange, null: false, foreign_key: true
      t.references :base_asset, null: false, foreign_key: { to_table: :assets }
      t.references :quote_asset, null: false, foreign_key: { to_table: :assets }
      t.string :ticker, null: false
      t.string :base, null: false
      t.string :quote, null: false
      t.string :minimum_base_size, null: false
      t.string :minimum_quote_size, null: false
      t.string :maximum_base_size, null: false
      t.string :maximum_quote_size, null: false
      t.integer :base_decimals, null: false
      t.integer :quote_decimals, null: false
      t.integer :price_decimals, null: false
      t.timestamps

      t.index [:exchange_id, :base_asset_id, :quote_asset_id], unique: true, name: 'index_exchange_tickers_on_unique_base_asset_and_quote_asset'
      t.index [:exchange_id, :base, :quote], unique: true, name: 'index_exchange_tickers_on_unique_base_and_quote'
      t.index [:exchange_id, :ticker], unique: true, name: 'index_exchange_tickers_on_unique_ticker'
    end
  end
end
