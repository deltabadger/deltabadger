class AddEthAddressToSubscriptions < ActiveRecord::Migration[6.0]
  def change
    add_column :subscriptions, :eth_address, :string
  end
end
