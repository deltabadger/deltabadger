class CreateAffiliates < ActiveRecord::Migration[5.2]
  def change
    create_table :affiliates do |t|
      t.belongs_to :user, index: { unique: true }, foreign_key: true
      t.integer :type, null: false
      t.string :name
      t.string :address
      t.string :vat_number
      t.string :btc_address, null: false
      t.string :code, null: false, index: { unique: true }
      t.string :visible_name
      t.string :visible_link
      t.decimal :max_profit, precision: 12, scale: 2, null: false
      t.decimal :discount_percent, precision: 3, scale: 2, null: false
      t.decimal :total_bonus_percent, precision: 3, scale: 2, null: false
      t.timestamps
    end
  end
end
