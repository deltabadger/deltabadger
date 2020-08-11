class CreateConversionRates < ActiveRecord::Migration[5.2]
  def change
    create_table :conversion_rates do |t|
      t.string :currency, null: false, index: { unique: true }
      t.decimal :rate, null: false
      t.timestamps
    end
  end
end
