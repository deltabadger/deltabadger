class AddOldCodeToAffiliates < ActiveRecord::Migration[5.2]
  def change
    add_column :affiliates, :old_code, :string
    change_column :affiliates, :btc_address, :string, null: true
  end
end
