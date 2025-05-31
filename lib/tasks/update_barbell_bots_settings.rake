desc 'rake task to update barbell bots settings'
task update_barbell_bots_settings: :environment do
  Bot.not_legacy.find_each do |bot|
    next if bot.settings.blank?

    puts "Updating bot #{bot.id}"
    settings = bot.settings
    if settings['price_limit_in_ticker_id'].present?
      ticker = ExchangeTicker.find(settings['price_limit_in_ticker_id'])
      ticker = bot.tickers.first unless bot.tickers.pluck(:id).include?(ticker.id)
      settings['price_limit_in_asset_id'] = ticker.base_asset_id
      vs_currency = ticker.quote_asset.symbol.downcase
      settings['price_limit_vs_currency'] = vs_currency.in?(Asset::VS_CURRENCIES) ? vs_currency : 'usd'
      settings.delete('price_limit_in_ticker_id')
    end
    bot.set_missed_quote_amount
    bot.update!(settings: settings.compact)
  end
end
