desc 'rake task to upgrade legacy bots'
task upgrade_legacy_bots: :environment do
  exchange_ids = Exchange.where(name_id: %w[coinbase kraken]).pluck(:id)
  bot_ids = Bot.basic
               .not_deleted
               .where(exchange_id: exchange_ids)
               .where('settings @> ?', { type: 'buy' }.to_json)
               .where('settings @> ?', { order_type: 'market' }.to_json)
               .where('settings @> ?', { price_range_enabled: false }.to_json)
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
      puts "Bot #{bot.id} has unknown settings: #{bot.settings.keys.reject { |key| known_settings.include?(key) }}"
      next
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
    smart_interval_quote_amount = bot.settings['smart_intervals_value'].to_f
    price_limit = bot.settings['price_range'][0].zero? ? nil : bot.settings['price_range'][0].to_f

    new_settings = {
      base_asset_id: base_asset.id,
      quote_asset_id: quote_asset.id,
      quote_amount: quote_amount,
      interval: interval,
      price_limit: price_limit,
      smart_interval_quote_amount: smart_interval_quote_amount
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
      started_at = NextTradingBotTransactionAt.new.call(bot) - 1.public_send(bot.interval)
      StopBot.call(bot.id)
      stopped_at = Time.current
      bot.reload
    else
      started_at = bot.last_submitted_transaction&.created_at
    end

    bot.settings = dummy_bot.settings
    bot.type = 'Bots::DcaSingleAsset'
    bot.save!

    bot = Bot.find(bot_id)

    if is_working
      bot.update!(stopped_at: stopped_at, started_at: started_at)
      amount_to_buy = bot.pending_quote_amount
      if amount_to_buy > bot.settings['quote_amount']
        raise "Amount to buy for bot #{bot.id} would be #{amount_to_buy} but quote amount is #{bot.settings['quote_amount']}"
      end

      puts "Starting bot #{bot.id}"
      bot.start(start_fresh: false)
    end

    # then update all transactions
    puts "Updating transactions base and quote for bot #{bot.id}"
    bot.transactions.find_each do |transaction|
      transaction.update!(
        base: ticker.base_asset.symbol,
        quote: ticker.quote_asset.symbol
      )
    end

    api_key = bot.user.api_keys.trading.find_by(exchange: bot.exchange)
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
          puts "Error updating transaction #{transaction.id} quote amount for bot #{bot.id}: #{result.error}"
        end
      end
    end
  end
end
