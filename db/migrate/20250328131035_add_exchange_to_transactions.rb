class AddExchangeToTransactions < ActiveRecord::Migration[6.0]
  def up
    add_reference :transactions, :exchange, foreign_key: true

    Transaction.find_each do |transaction|
      transaction.update_column(:exchange_id, transaction.bot.exchange_id)
    end
  end

  def down
    remove_reference :transactions, :exchange, foreign_key: true
  end
end
