class CreateBotIndexAssets < ActiveRecord::Migration[8.1]
  def change
    create_table :bot_index_assets do |t|
      t.references :bot, null: false, foreign_key: true
      t.references :asset, null: false, foreign_key: true
      t.references :ticker, null: false, foreign_key: true
      t.decimal :target_allocation, precision: 10, scale: 6
      t.decimal :current_allocation, precision: 10, scale: 6
      t.boolean :in_index, default: true
      t.datetime :entered_at
      t.datetime :exited_at
      t.timestamps
    end

    add_index :bot_index_assets, [:bot_id, :asset_id], unique: true
  end
end
