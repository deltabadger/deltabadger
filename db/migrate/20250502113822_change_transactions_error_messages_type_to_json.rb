class ChangeTransactionsErrorMessagesTypeToJson < ActiveRecord::Migration[6.0]
  def up
    # First handle any special cases
    change_column_default :transactions, :error_messages, from: "[]", to: nil
    
    # Fix Ruby symbol format in error_messages before converting type
    Transaction.find_each do |transaction|
      # Skip if already valid JSON or empty
      next if transaction.error_messages.blank? || transaction.error_messages == "[]"
      
      begin
        # If it can be parsed as JSON, it's already good
        JSON.parse(transaction.error_messages)
      rescue JSON::ParserError
        # If it contains Ruby symbols, eval it to get the Ruby array, then convert to JSON
        begin
          ruby_array = eval(transaction.error_messages)
          json_array = ruby_array.map { |item| item.is_a?(Symbol) ? item.to_s : item }.to_json
          transaction.update_column(:error_messages, json_array)
        rescue => e
          # If we can't eval it or something else goes wrong, set to empty array
          transaction.update_column(:error_messages, "[]")
        end
      end
    end
    
    # Now convert nil records to empty array
    Transaction.where(error_messages: "[nil]").update_all(error_messages: "[null]")
    
    # Finally convert the column type
    change_column :transactions, :error_messages, :jsonb, default: [], null: false, using: 'error_messages::jsonb'
  end

  def down
    change_column :transactions, :error_messages, :string, default: "[]", null: true
  end
end
