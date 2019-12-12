class AddBillingInfoToPayments < ActiveRecord::Migration[5.2]
  def change
    add_column :payments, :first_name, :string
    add_column :payments, :last_name, :string
    add_column :payments, :birth_date, :date
    add_column :payments, :eu, :boolean
  end
end
