class AddErrorMessagesJsonToTransactions < ActiveRecord::Migration[6.0]
  def change
    add_column :transactions, :error_messages_json, :jsonb, default: [], null: false
    change_column :transactions, :error_messages, :string, default: "[]", null: true
  end
end
