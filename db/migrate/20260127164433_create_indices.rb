class CreateIndices < ActiveRecord::Migration[8.1]
  def change
    create_table :indices do |t|
      t.string :external_id
      t.string :source
      t.string :name
      t.text :description
      t.json :top_coins
      t.integer :coins_count
      t.decimal :market_cap

      t.timestamps
    end
    add_index :indices, [:external_id, :source], unique: true
  end
end
