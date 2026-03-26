class CreateHistoricalPrices < ActiveRecord::Migration[8.1]
  def change
    create_table :historical_prices do |t|
      t.string :asset, null: false
      t.string :currency, null: false
      t.date :date, null: false
      t.decimal :price, null: false
    end

    add_index :historical_prices, %i[asset currency date], unique: true
  end
end
