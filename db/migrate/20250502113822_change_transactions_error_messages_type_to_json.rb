class ChangeTransactionsErrorMessagesTypeToJson < ActiveRecord::Migration[6.0]
  def up
    Transaction.where(error_messages: "[nil]").update_all(error_messages: "[null]")
    # change_column_default :transactions, :error_messages, from: "[]", to: nil
    # change_column :transactions, :error_messages, :jsonb, default: [], null: false, using: 'error_messages::jsonb'
  end

  def down
    # change_column :transactions, :error_messages, :string, default: "[]", null: true
  end
end
