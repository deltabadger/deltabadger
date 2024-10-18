class AddGadsTrackedToPayments < ActiveRecord::Migration[6.0]
  def change
    add_column :payments, :gads_tracked, :boolean, default: false
  end
end
