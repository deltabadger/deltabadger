class AddErrorMessagesJsonToTransactions < ActiveRecord::Migration[6.0]
  def change
    add_column :transactions, :error_messages_json, :jsonb, default: [], null: false
  end
end
