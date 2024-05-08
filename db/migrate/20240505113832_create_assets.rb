class CreateAssets < ActiveRecord::Migration[6.0]
  def change
    create_table :assets do |t|
      t.references :portfolio, null: false, foreign_key: true
      t.string :ticker
      t.float :allocation, default: 0.0, null: false

      t.timestamps
    end
  end
end
