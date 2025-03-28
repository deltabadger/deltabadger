class UpdateExchangeAssets < ActiveRecord::Migration[6.0]
  def up
    drop_table :exchange_assets

    create_table :exchange_assets do |t|
      t.belongs_to :asset, null: false, foreign_key: true
      t.belongs_to :exchange, null: false, foreign_key: true
      t.timestamps

      t.index [:asset_id, :exchange_id], unique: true
    end
  end

  def down
    drop_table :exchange_assets

    create_table :exchange_assets do |t|
      t.references :exchange, null: false, foreign_key: true
      t.string :ticker, null: false
      t.string :name
      t.string :color

      t.index [:exchange_id, :ticker], unique: true
    end
  end
end
