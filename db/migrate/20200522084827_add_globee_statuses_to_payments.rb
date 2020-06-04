class AddGlobeeStatusesToPayments < ActiveRecord::Migration[5.2]
  def change
    add_column :payments, :globee_statuses, :string, null: false, default: ''
  end
end
