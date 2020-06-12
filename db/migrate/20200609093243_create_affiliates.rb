class CreateAffiliates < ActiveRecord::Migration[5.2]
  def change
    create_table :affiliates do |t|
      t.belongs_to :user, index: { unique: true }, foreign_key: true
      t.string :first_name, null: false
      t.string :last_name, null: false
      t.date :birth_date
      t.boolean :eu
      t.string :btc_address, null: false
      t.string :code, null: false, index: { unique: true }
      t.decimal :max_profit, precision: 12, scale: 2, null: false
      t.decimal :profit_percent, precision: 2, scale: 2, null: false
      t.decimal :discount_percent, precision: 2, scale: 2, null: false
      t.timestamps
    end
  end
end
