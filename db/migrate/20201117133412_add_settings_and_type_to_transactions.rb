class AddSettingsAndTypeToTransactions < ActiveRecord::Migration[5.2]
  def change
    add_column :transactions, :bot_price, :decimal, precision: 20, scale: 10, null: false, default:"0.0"
    add_column :transactions, :bot_interval, :string, null: false, default: ""
    add_column :transactions, :transaction_type, :string, null: false, default: "REGULAR"

    reversible do |dir|
      dir.up do
        execute <<~SQL
          UPDATE transactions
          SET 
            bot_price = (settings->>'price')::decimal ,
            bot_interval = (settings->>'interval')
          FROM bots WHERE bots.id = transactions.bot_id;
        SQL
      end
    end
  end
end
