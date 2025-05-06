class FinishTransactionsErrorMessagesAsJson < ActiveRecord::Migration[6.0]
  def change
    remove_column :transactions, :error_messages, :string, default: "[]"
    remove_column :daily_transaction_aggregates, :error_messages, :string, default: "[]"

    rename_column :transactions, :error_messages_json, :error_messages
    rename_column :daily_transaction_aggregates, :error_messages_json, :error_messages
  end
end
