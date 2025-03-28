desc 'rake task to migrate barbell bots settings to new asset ids'
task migrate_barbell_bots_settings: :environment do
  Bot.barbell.find_each do |bot|
    ticker0 = bot.exchange&.tickers&.find_by(base: bot.base0, quote: bot.quote)
    ticker1 = bot.exchange&.tickers&.find_by(base: bot.base1, quote: bot.quote)
    next if ticker0.blank? || ticker1.blank?

    bot.update!(base0_asset_id: ticker0.base_asset_id,
                base1_asset_id: ticker1.base_asset_id,
                quote_asset_id: ticker0.quote_asset_id)
    puts "Bot #{bot.id} migrated"
  end
end
