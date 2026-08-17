class CreateFxRates < ActiveRecord::Migration[8.1]
  def change
    create_table :fx_rates do |t|
      t.string :currency, null: false
      t.date :date, null: false
      t.decimal :rate, null: false
    end
    add_index :fx_rates, %i[currency date], unique: true
  end
end
