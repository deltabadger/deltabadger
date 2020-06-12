class CreateAffiliates < ActiveRecord::Migration[5.2]
  def change
    create_table :affiliates do |t|
      t.belongs_to :user, index: { unique: true }, foreign_key: true
      t.string :first_name, null: false
      t.string :last_name, null: false
      t.date :birth_date, null: false
      t.boolean :eu, null: false
      t.string :btc_address, null: false
      t.string :code, null: false, index: { unique: true }
      t.decimal :max_profit, precision: 12, scale: 2, null: false
      t.decimal :discount_percent, precision: 3, scale: 2, null: false
      t.decimal :total_bonus_percent, precision: 3, scale: 2, null: false
      t.timestamps
    end
  end
end
