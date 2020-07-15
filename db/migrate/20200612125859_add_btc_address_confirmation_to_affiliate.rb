class AddBtcAddressConfirmationToAffiliate < ActiveRecord::Migration[5.2]
  def change
    add_column :affiliates, :new_btc_address, :string
    add_column :affiliates, :new_btc_address_token, :string
    add_column :affiliates, :new_btc_address_send_at, :datetime

    add_index :affiliates, :new_btc_address_token, unique: true
  end
end
