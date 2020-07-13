class AddDiscountedToPayments < ActiveRecord::Migration[5.2]
  def change
    add_column :payments, :discounted, :boolean, null: false, default: false
  end
end
