class UpdateVatRate < ActiveRecord::Migration[6.0]
  def change
    rename_table :vat_rates, :countries

    add_column :countries, :code, :string, index: { unique: true }
    add_column :countries, :eu_member, :boolean, default: false, null: false
    add_column :countries, :currency, :integer, default: 0, null: false
    rename_column :countries, :vat, :vat_rate
    rename_column :countries, :country, :name
  end
end
