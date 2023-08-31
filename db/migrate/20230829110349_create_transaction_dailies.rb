class CreateTransactionDailies < ActiveRecord::Migration[5.2]
  def change
    create_table :transaction_dailies do |t|
      t.references :bot, foreign_key: true
      t.string :offer_id
      t.decimal :rate
      t.decimal :amount
      t.string :market
      t.integer :status
      t.integer :currency
      t.string :error_messages, default: "[]"
      t.decimal :bot_price, precision: 20, scale: 10, default: "0.0", null: false
      t.string :bot_interval, default: "", null: false
      t.string :transaction_type, default: "REGULAR", null: false
      t.string :called_bot_type
      t.timestamps
    end

    add_index :transaction_dailies, [:bot_id, :transaction_type, :created_at], name: 'dailies_index_bot_type_created_at'
    add_index :transaction_dailies, [:bot_id, :created_at]
    add_index :transaction_dailies, [:bot_id, :status, :created_at]
  end
end
