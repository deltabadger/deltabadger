class AddErrorsToTransaction < ActiveRecord::Migration[5.2]
  def change
    add_column :transactions, :error_messages, :string, default: '[]'
  end
end
