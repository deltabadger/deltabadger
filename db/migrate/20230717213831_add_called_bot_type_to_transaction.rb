class AddCalledBotTypeToTransaction < ActiveRecord::Migration[6.0]
  def change
    add_column :transactions, :called_bot_type, :string
  end
end
