module BotHelper
  def bot_intervals
    Bot::INTERVALS.map { |interval| [t("bot.#{interval}"), interval] }
  end

  def bot_type_label(bot)
    {
      'Bots::DcaDualAsset' => 'Barbell DCA',
      'Bots::Basic' => 'Basic DCA',
      'Bots::Withdrawal' => 'Withdrawal',
      'Bots::Webhook' => 'Webhook'
    }[bot.type]
  end

  def price_limit_timing_condition_select_options(bot)
    return [] unless defined?(bot.class::PRICE_LIMIT_TIMING_CONDITIONS)

    bot.class::PRICE_LIMIT_TIMING_CONDITIONS.map do |condition|
      [t("bot.settings.extra_price_limit.timing_condition.#{condition}"), condition]
    end
  end

  def price_limit_value_condition_select_options(bot)
    return [] unless defined?(bot.class::PRICE_LIMIT_VALUE_CONDITIONS)

    bot.class::PRICE_LIMIT_VALUE_CONDITIONS.map do |condition|
      [t("bot.settings.extra_price_limit.price_condition.#{condition}"), condition]
    end
  end

  def base_select_options(bot)
    bot.tickers.pluck(:base_asset_id, :id).map do |base_asset_id, id|
      [Asset.find(base_asset_id).symbol, id]
    end.sort_by(&:first)
  end

  def vs_currencies_select_options
    Asset::VS_CURRENCIES.map do |currency|
      [currency.upcase, currency]
    end
  end
end
