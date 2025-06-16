desc 'rake task to update new bots transactions data'
task update_new_bots_transactions_data: :environment do
  # loop do
  update_new_bots_transactions_remote_data
  # end
end

def update_new_bots_transactions_remote_data
  puts 'updating transactions remote data'
  Bot.not_legacy.find_each do |bot|
    puts "updating transactions for bot #{bot.id}"
    api_key = bot.user.api_keys.trading.correct.find_by(exchange: bot.exchange)
    next if api_key.blank?

    bot.exchange.set_client(api_key: api_key)
    bot.transactions.submitted
       .where.not(external_id: nil)
       .where(quote_amount_exec: nil)
       .order(created_at: :desc).each do |transaction|
      puts "getting order #{transaction.external_id} (#{transaction.created_at})"
      begin
        result = bot.exchange.get_order(order_id: transaction.external_id)
      rescue KeyError => e
        puts "error getting order for #{transaction.external_id} (#{transaction.created_at}): #{e.message}"
        next
      end
      if result.failure?
        puts "failure getting order for #{transaction.external_id} (#{transaction.created_at}): #{result.errors.to_sentence}"
        next
      end

      puts "updating transaction #{transaction.id}"
      order_data = result.data
      order_values = {
        external_status: order_data[:status],
        price: order_data[:price],
        amount: order_data[:amount],
        quote_amount: order_data[:quote_amount],
        base: order_data[:ticker].base_asset.symbol,
        quote: order_data[:ticker].quote_asset.symbol,
        side: order_data[:side],
        order_type: order_data[:order_type],
        amount_exec: order_data[:amount_exec],
        quote_amount_exec: order_data[:quote_amount_exec]
      }
      transaction.update!(order_values)
    end
  end
end
