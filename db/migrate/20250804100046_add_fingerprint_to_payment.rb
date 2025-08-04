class AddFingerprintToPayment < ActiveRecord::Migration[6.0]
  def change
    add_column :payments, :finger_print_id, :string
  end
end
