class AddErrorMessagesJsonToDailyTransactionAggregates < ActiveRecord::Migration[6.0]
  def change
    add_column :daily_transaction_aggregates, :error_messages_json, :jsonb, default: [], null: false
  end
end
