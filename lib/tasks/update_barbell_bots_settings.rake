desc 'rake task to update barbell bots settings'
task update_barbell_bots_settings: :environment do
  Bot.not_legacy.find_each do |bot|
    next if bot.settings.blank?

    puts "Updating bot #{bot.id}"
    settings = bot.settings

    # settings['price_limit_in_asset_id'] = bot.tickers&.first&.id
    # settings.delete('price_limit_in_asset_id') if settings['price_limit_in_asset_id'].present?
    # settings.delete('price_limit_vs_currency') if settings['price_limit_vs_currency'].present?

    if settings['indicator_limit_in_timeframe'].present?
      settings['indicator_limit_in_timeframe'] = 'one_day'
      # settings.delete('indicator_limit_in_timeframe')
    end

    bot.set_missed_quote_amount
    bot.update!(settings: settings.compact)
  end
end
