class AddIndexesToEnums < ActiveRecord::Migration[6.0]
  def change
    add_index :payments, :currency
    add_index :payments, :status
    add_index :payments, :payment_type
  end
end
