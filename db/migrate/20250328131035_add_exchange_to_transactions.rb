class AddExchangeToTransactions < ActiveRecord::Migration[6.0]
  def up
    add_reference :transactions, :exchange, foreign_key: true

    Bot.pluck(:id, :exchange_id).sort.each do |id, exchange_id|
      Rails.logger.info "Updating transactions for bot #{id} with exchange #{exchange_id}"
      puts "Updating transactions for bot #{id} with exchange #{exchange_id}"
      Transaction.where(bot_id: id).update_all(exchange_id: exchange_id)
    end

    change_column_null :transactions, :exchange_id, false
  end

  def down
    remove_reference :transactions, :exchange, foreign_key: true
  end
end
