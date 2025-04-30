class AddExchangeToTransactions < ActiveRecord::Migration[6.0]
  def up
    add_reference :transactions, :exchange, foreign_key: true

    # Use parallel processing with multiple threads
    require 'parallel'

    # Fetch bot data
    bot_data = Bot.pluck(:id, :exchange_id).sort

    # Process in parallel using available cores
    Parallel.each(bot_data, in_threads: ENV.fetch('MAX_DB_CONNECTIONS', 1)) do |id, exchange_id|
      Rails.logger.info "Updating transactions for bot #{id} with exchange #{exchange_id}"
      puts "Updating transactions for bot #{id} with exchange #{exchange_id}"

      # Update in batches to reduce memory usage and transaction overhead
      Transaction.where(bot_id: id).in_batches.update_all(exchange_id: exchange_id)
    end

    change_column_null :transactions, :exchange_id, false
  end

  def down
    remove_reference :transactions, :exchange, foreign_key: true
  end
end
