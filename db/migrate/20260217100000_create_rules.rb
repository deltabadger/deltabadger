class CreateRules < ActiveRecord::Migration[8.1]
  def change
    create_table :rules do |t|
      t.string :type, null: false
      t.references :user, null: false, foreign_key: true
      t.references :exchange, foreign_key: true
      t.references :asset, foreign_key: { to_table: :assets }
      t.integer :status, default: 0, null: false
      t.json :settings, default: {}, null: false
      t.string :address
      t.timestamps
    end
    add_index :rules, [:user_id, :type, :exchange_id, :asset_id], unique: true,
              name: "idx_rules_user_type_exchange_asset"
  end
end
