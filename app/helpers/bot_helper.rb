module BotHelper
  def bot_intervals
    # FIXME: reenable monthly once smart intervals are back on
    Bot::INTERVALS[...-1].map { |interval| [t("bot.#{interval}"), interval] }
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
    return [] unless defined?(bot.class::TIMING_CONDITIONS)

    bot.class::TIMING_CONDITIONS.map do |condition|
      [t("bot.settings.extra_price_limit.timing_condition.#{condition}"), condition]
    end
  end

  def price_limit_price_condition_select_options(bot)
    return [] unless defined?(bot.class::PRICE_CONDITIONS)

    bot.class::PRICE_CONDITIONS.map do |condition|
      [t("bot.settings.extra_price_limit.price_condition.#{condition}"), condition]
    end
  end

  def base_select_options(bot)
    bot.tickers.pluck(:base, :id).sort
  end
end
