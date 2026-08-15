class AddInstrumentTypeToAssets < ActiveRecord::Migration[8.1]
  def change
    add_column :assets, :instrument_type, :string
  end
end
