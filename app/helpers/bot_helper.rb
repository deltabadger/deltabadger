module BotHelper
  def bot_intervals_select_options
    Bot::Schedulable::INTERVALS.keys.map { |interval| [t("bot.#{interval}"), interval] }
  end

  def bot_type_label(bot)
    {
      'Bots::DcaDualAsset' => 'Rebalanced DCA',
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
      next if condition == 'between' && bot.price_limit_timing_condition != 'while'

      [t("bot.settings.extra_price_limit.value_condition.#{condition}"), condition]
    end.compact
  end

  def price_drop_limit_time_window_condition_select_options(bot)
    return [] unless defined?(bot.class::PRICE_DROP_LIMIT_TIME_WINDOW_CONDITIONS)

    bot.class::PRICE_DROP_LIMIT_TIME_WINDOW_CONDITIONS.keys.map do |condition|
      [t("bot.settings.extra_price_drop_limit.time_window_condition.#{condition}"), condition]
    end
  end

  def base_select_options(bot)
    bot.tickers.pluck(:base_asset_id, :id).map do |base_asset_id, id|
      [Asset.find(base_asset_id).symbol, id]
    end.sort_by(&:first)
  end

  def indicator_limit_timeframe_select_options
    Bot::IndicatorLimitable::INDICATOR_LIMIT_TIMEFRAMES
      .sort_by { |_, duration| duration }
      .map { |locale_key, _| [t("bot.settings.extra_indicator_limit.timeframe.#{locale_key}"), locale_key] }
  end

  def indicator_limit_timing_condition_select_options(bot)
    return [] unless defined?(bot.class::INDICATOR_LIMIT_TIMING_CONDITIONS)

    bot.class::INDICATOR_LIMIT_TIMING_CONDITIONS.map do |condition|
      [t("bot.settings.extra_indicator_limit.timing_condition.#{condition}"), condition]
    end
  end

  def indicator_limit_value_condition_select_options(bot)
    return [] unless defined?(bot.class::INDICATOR_LIMIT_VALUE_CONDITIONS)

    bot.class::INDICATOR_LIMIT_VALUE_CONDITIONS.map do |condition|
      [t("bot.settings.extra_indicator_limit.value_condition.#{condition}"), condition]
    end
  end

  def moving_average_limit_timeframe_select_options
    Bot::MovingAverageLimitable::MOVING_AVERAGE_LIMIT_TIMEFRAMES
      .sort_by { |_, duration| duration }
      .map { |locale_key, _| [t("bot.settings.extra_moving_average_limit.timeframe.#{locale_key}"), locale_key] }
  end

  def moving_average_limit_timing_condition_select_options(bot)
    return [] unless defined?(bot.class::MOVING_AVERAGE_LIMIT_TIMING_CONDITIONS)

    bot.class::MOVING_AVERAGE_LIMIT_TIMING_CONDITIONS.map do |condition|
      [t("bot.settings.extra_moving_average_limit.timing_condition.#{condition}"), condition]
    end
  end

  def moving_average_limit_value_condition_select_options(bot)
    return [] unless defined?(bot.class::MOVING_AVERAGE_LIMIT_VALUE_CONDITIONS)

    bot.class::MOVING_AVERAGE_LIMIT_VALUE_CONDITIONS.map do |condition|
      [t("bot.settings.extra_indicator_limit.value_condition.#{condition}"), condition]
    end
  end

  def ticker_select_options(bot)
    bot.tickers.pluck(:id, :base_asset_id, :quote_asset_id).map do |id, base_asset_id, quote_asset_id|
      ["#{Asset.find(base_asset_id).symbol}#{Asset.find(quote_asset_id).symbol}", id]
    end.sort_by(&:first)
  end

  def indicator_select_options
    Bot::IndicatorLimitable::INDICATOR_LIMIT_INDICATORS.map do |indicator|
      [indicator.upcase, indicator]
    end
  end

  def moving_average_select_options
    Bot::MovingAverageLimitable::MOVING_AVERAGE_LIMIT_MA_TYPES.map do |ma_type|
      [ma_type.upcase, ma_type]
    end
  end

  def vs_currencies_select_options
    Asset::VS_CURRENCIES.map do |currency|
      [currency.upcase, currency]
    end
  end
end
