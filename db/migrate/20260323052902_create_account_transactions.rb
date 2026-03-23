class CreateAccountTransactions < ActiveRecord::Migration[8.1]
  def change
    create_table :account_transactions do |t|
      t.references :api_key, null: false, foreign_key: true
      t.references :exchange, null: false, foreign_key: true
      t.integer :entry_type, null: false
      t.string :base_currency, null: false
      t.decimal :base_amount, null: false
      t.string :quote_currency
      t.decimal :quote_amount
      t.string :fee_currency
      t.decimal :fee_amount
      t.string :tx_id
      t.string :group_id
      t.string :description
      t.datetime :transacted_at, null: false
      t.json :raw_data, default: {}
      t.references :transaction, foreign_key: true

      t.timestamps
    end

    add_index :account_transactions, [:exchange_id, :tx_id], unique: true, where: "tx_id IS NOT NULL"
    add_index :account_transactions, [:api_key_id, :transacted_at]
    add_index :account_transactions, :transacted_at
    add_index :account_transactions, :group_id

    add_column :api_keys, :last_synced_at, :datetime
  end
end
