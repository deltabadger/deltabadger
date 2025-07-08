class AddRecurringFlagToPayment < ActiveRecord::Migration[6.0]
  def change
    add_column :payments, :recurring, :boolean, default: false, null: false
  end
end
