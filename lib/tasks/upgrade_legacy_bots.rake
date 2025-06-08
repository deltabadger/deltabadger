desc 'rake task to upgrade legacy bots'
task upgrade_legacy_bots: :environment do

  exchange_ids = Exchange.where(name_id: ["coinbase", "kraken"]).pluck(:id)
  bots = Bot.basic.not_deleted.where(exchange_id: exchange_ids)
                              .where("settings @> ?", {type: "buy"}.to_json)
                              .where("settings @> ?", {order_type: "market"}.to_json)
                              .where("settings @> ?", {price_range_enabled: false}.to_json)

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

  bots.find_each do |bot|
    puts "Updating bot #{bot.id}"

    if bot.settings.keys.all? { |key| known_settings.include?(key) }
      puts "Bot #{bot.id} has unknown settings: #{bot.settings.keys.join(', ')}"
      next
    end

    ticker = ExchangeTicker.find_by(exchange: bot.exchange, base: bot.settings['base'], quote: bot.settings['quote'])
    if ticker.blank?
      puts "Ticker not found for bot #{bot.id}"
      next
    end

    base_asset = ticker.base_asset
    quote_asset = ticker.quote_asset
    quote_amount = bot.settings['price'].to_d
    interval = bot.settings['interval']
    smart_interval_quote_amount = bot.settings['smart_intervals_value'].to_d
    price_limit = bot.settings['price_range'][0].zero? ? nil : bot.settings['price_range'][0].to_d

    new_settings = {
      base_asset_id: base_asset.id,
      quote_asset_id: quote_asset.id,
      quote_amount: quote_amount,
      interval: interval,
      price_limit: price_limit,
      smart_interval_quote_amount: smart_interval_quote_amount,
    }.compact

    # use the dummy bot to initialize all other settings
    dummy_bot = Bot.dca_single_asset.new(
      exchange: bot.exchange,
      settings: new_settings
    )

    bot.settings = dummy_bot.settings
    bot.set_missed_quote_amount
    bot.save!

    # then update all transactions
    puts "Updating transactions base and quote for bot #{bot.id}"
    bot.transactions.find_each do |transaction|
      transaction.update!(
        base: ticker.base_asset.symbol,
        quote: ticker.quote_asset.symbol,
      )
    end

    api_key = bot.user.api_keys.find_by(exchange: bot.exchange)
    if api_key.blank?
      puts "No api key found for bot #{bot.id}. Could not update transactions quote amount"
      next
    end

    bot.transactions.submitted.find_each do |transaction|
      puts "Updating transaction #{transaction.id} quote amount for bot #{bot.id}"
      bot.with_api_key do
        result = bot.exchange.get_order(order_id: transaction.external_id)
        if result.success?
          transaction.update!(quote_amount: result.data[:quote_amount])
        else
          puts "Error updating transaction #{transaction.id} quote amount for bot #{bot.id}: #{result.error}"
        end
      end
    end
  end
end
