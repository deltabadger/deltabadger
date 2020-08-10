class AddCountryToPayments < ActiveRecord::Migration[5.2]
  def change
    add_column :payments, :country, :string

    reversible do |dir|
      dir.up do
        execute <<~SQL
          UPDATE payments
            SET country =
              (CASE WHEN eu THEN 'EU' ELSE 'Other' END);
        SQL
      end

      dir.down do
        execute <<~SQL
          UPDATE payments
            SET eu =
              (CASE WHEN country = 'Other' THEN false ELSE true END);
        SQL
      end
    end

    change_column_null :payments, :country, false
    remove_column :payments, :eu, :boolean
  end
end
