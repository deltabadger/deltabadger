desc 'rake task to upgrade legacy bots'
task upgrade_legacy_bots: :environment do
  exchange_ids = Exchange.where(name_id: %w[coinbase kraken]).pluck(:id)
  bot_ids = Bot.basic
               .not_deleted
               .where(exchange_id: exchange_ids)
               .where('settings @> ?', { type: 'buy' }.to_json)
               .where('settings @> ?', { order_type: 'market' }.to_json)
               .pluck(:id)

  known_settings = %w[
    base
    type
    price
    quote
    interval
    order_type
    percentage
    price_range
    use_subaccount
    price_range_enabled
    selected_subaccount
    force_smart_intervals
    smart_intervals_value
  ]

  bot_ids.sort.each do |bot_id|
    bot = Bot.find(bot_id)
    puts "Updating bot #{bot.id} with settings #{bot.settings.inspect}"

    unless bot.settings.keys.all? { |key| known_settings.include?(key) }
      raise "Bot #{bot.id} has unknown settings: #{bot.settings.keys.reject { |key| known_settings.include?(key) }}"
    end

    ticker = ExchangeTicker.find_by(exchange: bot.exchange, base: bot.settings['base'], quote: bot.settings['quote'])
    if ticker.blank?
      puts "Ticker not found for bot #{bot.id}"
      next
    end

    base_asset = ticker.base_asset
    quote_asset = ticker.quote_asset
    quote_amount = bot.settings['price'].to_f
    interval = bot.settings['interval']
    smart_intervaled = bot.settings['force_smart_intervals']

    if bot.exchange.name_id == 'kraken'
      result = ticker.get_last_price
      raise "Error getting last price for bot #{bot.id}: #{result.errors.to_sentence}" unless result.success?

      price = result.data
      smart_interval_quote_amount = (bot.settings['smart_intervals_value'].to_f * price).ceil(ticker.quote_decimals).to_f
    else
      smart_interval_quote_amount = bot.settings['smart_intervals_value'].to_f
    end

    price_limit_range_lower_bound = bot.settings['price_range'].map(&:to_f).min
    price_limit_range_upper_bound = bot.settings['price_range'].map(&:to_f).max
    price_limit = price_limit_range_upper_bound
    price_limited = bot.settings['price_range_enabled']
    price_limit_value_condition = 'between'

    new_settings = {
      base_asset_id: base_asset.id,
      quote_asset_id: quote_asset.id,
      quote_amount: quote_amount,
      interval: interval,
      price_limited: price_limited,
      price_limit: price_limit,
      price_limit_range_lower_bound: price_limit_range_lower_bound,
      price_limit_range_upper_bound: price_limit_range_upper_bound,
      price_limit_value_condition: price_limit_value_condition,
      smart_intervaled: smart_intervaled,
      smart_interval_quote_amount: [
        smart_interval_quote_amount,
        minimum_smart_interval_quote_amount(quote_amount, interval, ticker)
      ].max.to_f
    }.compact

    # use the dummy bot to initialize all other settings
    dummy_bot = Bot.dca_single_asset.new(
      exchange: bot.exchange,
      settings: new_settings
    )

    is_working = bot.working?
    stopped_at = nil
    if is_working
      puts "Stopping bot #{bot.id}"
      next_transaction_at = NextTradingBotTransactionAt.new.call(bot)
      started_at = next_transaction_at - 1.public_send(bot.interval) if next_transaction_at.present?
      StopBot.call(bot.id)
      stopped_at = Time.current
      bot.reload
    else
      started_at = bot.last_successful_transaction&.created_at
    end

    bot.settings = dummy_bot.settings
    bot.type = 'Bots::DcaSingleAsset'
    bot.save!

    bot = Bot.find(bot_id)
    bot.update!(stopped_at: stopped_at, started_at: started_at)

    if is_working
      amount_to_buy = bot.pending_quote_amount
      if amount_to_buy > bot.settings['quote_amount']
        raise "Amount to buy for bot #{bot.id} would be #{amount_to_buy} but quote amount is #{bot.settings['quote_amount']}"
      end

      puts "Starting bot #{bot.id}"
      raise "Could not start bot #{bot.id}" unless bot.start(start_fresh: false)
    end

    # then update all transactions
    puts "Updating transactions base and quote for bot #{bot.id}"
    bot.transactions.where(base: nil).update_all(base: ticker.base_asset.symbol)
    bot.transactions.where(quote: nil).update_all(quote: ticker.quote_asset.symbol)

    api_key = bot.user.api_keys.correct.trading.find_by(exchange: bot.exchange)
    if api_key.blank?
      puts "No api key found for bot #{bot.id}. Could not update transactions quote amount"
      next
    end

    puts "checking valid api key for #{bot.exchange.name} for user #{bot.user.id}"
    result = bot.exchange.check_valid_api_key?(api_key: api_key)
    if result.failure?
      puts "failed to check valid api key for #{bot.exchange.name} for user #{bot.user.id}: #{result.errors.to_sentence}"
      next
    end

    valid = result.data
    if !valid
      puts "invalid api key for #{bot.exchange.name} for user #{bot.user.id}"
      next
    end

    bot.transactions.submitted.where(quote_amount: nil).find_each do |transaction|
      puts "Updating transaction #{transaction.id} quote amount for bot #{bot.id}"
      bot.with_api_key do
        result = bot.exchange.get_order(order_id: transaction.external_id)
        if result.success?

          quote_amount = result.data[:quote_amount] || (result.data[:price] * result.data[:amount]).to_d
          side = result.data[:side]
          order_type = result.data[:order_type]
          filled_percentage = result.data[:filled_percentage]
          external_status = result.data[:status]
          base = result.data[:ticker]&.base_asset&.symbol
          quote = result.data[:ticker]&.quote_asset&.symbol
          puts "updating transaction #{transaction.id}: #{base}#{quote} #{order_type} #{side}" \
               " #{quote_amount} - #{(filled_percentage * 100).round(2)}% filled [#{external_status}]"

          transaction.update!(
            price: result.data[:price],
            amount: result.data[:amount],
            base: base,
            quote: quote,
            quote_amount: quote_amount,
            side: side,
            order_type: order_type,
            filled_percentage: filled_percentage,
            external_status: external_status
          )
        else
          puts "Error updating transaction #{transaction.id} quote amount for bot #{bot.id}: #{result.errors.to_sentence}"
        end
      end
    end
  end
end

def minimum_smart_interval_quote_amount(quote_amount, interval, ticker)
  # the minimum amount would set one order every 1 minute
  maximum_frequency = 300 # seconds
  minimum_for_frequency = if quote_amount.present?
                            quote_amount / Bot::Schedulable::INTERVALS[interval] * maximum_frequency
                          else
                            0
                          end

  least_precise_quote_decimals = ticker.quote_decimals
  minimum_for_precision = 1.0 / (10**least_precise_quote_decimals)

  [
    Utilities::Number.round_up(minimum_for_frequency, precision: least_precise_quote_decimals),
    minimum_for_precision
  ].max
end
