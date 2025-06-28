desc 'rake task to update new bots transactions data'
task update_new_bots_transactions_data: :environment do
  loop do
    update_new_bots_transactions_remote_data
  end
end

def update_new_bots_transactions_remote_data
  puts 'updating transactions remote data'
  bot_ids = Bot.not_legacy.pluck(:id).sort.reverse
  bot_ids.each do |bot_id|
    bot = Bot.find(bot_id)
    bot.transactions.where(price: 0).update_all(price: nil)
    update_transactions_assets(bot) if bot.dca_single_asset?
    update_binance_external_ids(bot) if bot.exchange.type.in?(%w[Exchanges::Binance Exchanges::BinanceUs])
    api_key = bot.user.api_keys.trading.correct.find_by(exchange: bot.exchange)
    next if api_key.blank?

    bot.exchange.set_client(api_key: api_key)
    transaction_ids = bot.transactions.submitted
                         .where.not(external_id: nil)
                         .where(quote_amount_exec: nil)
                         .order(created_at: :desc)
                         .pluck(:external_id)
    next if transaction_ids.blank?

    puts "updating transactions for bot #{bot.id}"

    transaction_ids.each_slice(1000) do |transaction_ids_slice|
      begin
        puts "getting orders for #{transaction_ids_slice.first}..#{transaction_ids_slice.last} (#{transaction_ids_slice.size})"
        result = bot.exchange.get_orders(order_ids: transaction_ids_slice)
      rescue KeyError => e
        puts "error getting orders for #{transaction_ids_slice.first}..#{transaction_ids_slice.last} (#{transaction_ids_slice.size}): #{e.message}"
        break
      end
      if result.failure?
        puts "failure getting orders for #{transaction_ids_slice.first}..#{transaction_ids_slice.last} (#{transaction_ids_slice.size}): #{result.errors.to_sentence}"
        break
      end

      puts "updating transactions #{transaction_ids_slice.first}..#{transaction_ids_slice.last} (#{transaction_ids_slice.size})"
      orders = result.data
      break if orders.empty?

      orders.each do |order_id, order_data|
        transaction = bot.transactions.find_by(external_id: order_id)
        raise "transaction not found for #{order_id}" if transaction.nil?

        ticker = if bot.dca_single_asset?
                   order_data[:ticker] || bot.ticker
                 else
                   order_data[:ticker]
                 end

        if ticker.nil?
          puts "ticker is nil for #{transaction.external_id} (#{transaction.created_at})"
          # next
        end

        puts "updating transaction #{transaction.id}"

        order_values = {
          external_status: order_data[:status],
          price: order_data[:price],
          amount: order_data[:amount],
          quote_amount: order_data[:quote_amount],
          base: ticker&.base_asset&.symbol || transaction.base,
          quote: ticker&.quote_asset&.symbol || transaction.quote,
          side: order_data[:side],
          order_type: order_data[:order_type],
          amount_exec: order_data[:amount_exec],
          quote_amount_exec: order_data[:quote_amount_exec]
        }

        # do not convert the asset of transactions of bots migrated from BUSD to FDUSD
        if bot.exchange.type.in?(%w[Exchanges::Binance Exchanges::BinanceUs]) && transaction.quote == 'BUSD'
          order_values[:quote] = transaction.quote
        end

        transaction.update!(order_values)
      end
    end
  end
end

def update_transactions_assets(bot)
  return unless bot.dca_single_asset?

  ticker = bot.ticker
  bot.transactions.where(base: nil).update_all(base: ticker.base_asset.symbol, quote: ticker.quote_asset.symbol)
  bot.transactions.where(quote: nil).update_all(base: ticker.base_asset.symbol, quote: ticker.quote_asset.symbol)
end

def update_binance_external_ids(bot)
  return unless bot.exchange.type.in?(%w[Exchanges::Binance Exchanges::BinanceUs])

  bot.transactions.submitted.where.not(external_id: nil).find_each do |transaction|
    next if transaction.external_id.include?('-')

    if transaction.base.present? && transaction.quote.present?
      transaction.update!(external_id: "#{transaction.base}#{transaction.quote}-#{transaction.external_id}")
    else
      ticker = if bot.dca_single_asset?
                 bot.ticker
               else
                 bot.tickers.find_by(base_asset: transaction.base, quote_asset: transaction.quote)
               end
      raise "ticker not found for #{bot.id}" if ticker.nil?

      puts "updating binance external id for transaction #{transaction.id}"
      transaction.update!(external_id: "#{ticker.ticker}-#{transaction.external_id}")
    end
  end
end
