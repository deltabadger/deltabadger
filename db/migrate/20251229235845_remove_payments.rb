class RemovePayments < ActiveRecord::Migration[6.0]
  def change
    remove_column :users, :pending_wire_transfer
    drop_table :setting_flags
    drop_table :payments
  end
end
