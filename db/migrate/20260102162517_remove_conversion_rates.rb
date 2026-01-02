class RemoveConversionRates < ActiveRecord::Migration[6.0]
  def change
    drop_table :conversion_rates
  end
end
