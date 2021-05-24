class AddPendingWireTransferToUsers < ActiveRecord::Migration[5.2]
  def change
    add_column :users, :pending_wire_transfer, :string, default: nil
    add_column :users, :pending_plan_id, :integer, default: nil
  end
end
