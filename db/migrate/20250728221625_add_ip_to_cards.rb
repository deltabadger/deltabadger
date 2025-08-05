class AddIpToCards < ActiveRecord::Migration[6.0]
  def change
    add_column :cards, :ip, :string
    change_column_null :cards, :first_transaction_id, true
  end
end
