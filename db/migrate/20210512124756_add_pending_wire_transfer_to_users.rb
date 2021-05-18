class AddPendingWireTransferToUsers < ActiveRecord::Migration[5.2]
  def change
    add_column :users, :pending_wire_transfer, :integer, null: false, default: 0
  end
end
