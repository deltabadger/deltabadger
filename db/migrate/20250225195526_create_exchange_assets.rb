class CreateExchangeAssets < ActiveRecord::Migration[6.0]
  def change
    create_table :exchange_assets do |t|
      t.references :exchange, null: false, foreign_key: true
      t.string :ticker, null: false
      t.string :name
      t.string :color

      t.index [:exchange_id, :ticker], unique: true
    end
  end
end
