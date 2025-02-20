class UpdateTransactionColumns < ActiveRecord::Migration[6.0]
  def up
    remove_column :transactions, :market
    remove_column :transactions, :currency
    add_column :transactions, :base, :string
    add_column :transactions, :quote, :string

    remove_column :daily_transaction_aggregates, :market
    remove_column :daily_transaction_aggregates, :currency
    add_column :daily_transaction_aggregates, :base, :string
    add_column :daily_transaction_aggregates, :quote, :string
  end

  def down
    add_column :transactions, :market, :string
    add_column :transactions, :currency, :string
    remove_column :transactions, :base
    remove_column :transactions, :quote

    add_column :daily_transaction_aggregates, :market, :string
    add_column :daily_transaction_aggregates, :currency, :string
    remove_column :daily_transaction_aggregates, :base
    remove_column :daily_transaction_aggregates, :quote
  end
end
