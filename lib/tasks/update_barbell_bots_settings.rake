desc 'rake task to update barbell bots settings'
task update_barbell_bots_settings: :environment do
  Bot.dca_dual_asset.find_each do |bot|
    next if bot.settings.blank?

    puts "Updating bot #{bot.id}"
    settings = bot.settings
    if settings['market_cap_adjusted'].present?
      settings['marketcap_allocated'] = settings['market_cap_adjusted'].presence || false
      settings.delete('market_cap_adjusted')
    end
    bot.update!(settings: settings.compact)
  end
end
