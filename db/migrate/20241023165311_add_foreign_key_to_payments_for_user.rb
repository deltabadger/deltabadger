class AddForeignKeyToPaymentsForUser < ActiveRecord::Migration[6.0]
  def change
    add_foreign_key :payments, :users
  end
end
