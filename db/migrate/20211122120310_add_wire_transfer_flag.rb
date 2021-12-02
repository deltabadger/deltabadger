class AddWireTransferFlag < ActiveRecord::Migration[5.2]
  def change
    add_column :payments, :wire_transfer, :boolean, :default => false
  end
end
