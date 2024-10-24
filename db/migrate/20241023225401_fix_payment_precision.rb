class FixPaymentPrecision < ActiveRecord::Migration[6.0]
  def up
    change_column :payments, :total, :decimal, precision: 10, scale: 2
    change_column :payments, :commission, :decimal, precision: 10, scale: 2
  end

  def down
    change_column :payments, :total, :decimal, precision: nil, scale: nil
    change_column :payments, :commission, :decimal, precision: nil, scale: nil
  end
end
