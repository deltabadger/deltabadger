class RemoveWireTransferFieldFromPayments < ActiveRecord::Migration[5.2]
  def change
    add_column :payments, :payment_type, :integer, null: false, default: 0
    execute "UPDATE payments SET payment_type = 1 WHERE wire_transfer = true"
    remove_column :payments, :wire_transfer, :boolean
  end
end
