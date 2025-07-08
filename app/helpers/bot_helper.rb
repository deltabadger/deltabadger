module BotHelper
  def bot_intervals_select_options(subscription)
    if subscription.paid?
      Bot::Schedulable::INTERVALS.keys.map { |interval| [t("bot.#{interval}"), interval] }
    else
      Bot::Schedulable::INTERVALS.keys.map do |interval|
        if interval == 'day'
          [t("bot.#{interval}"), interval]
        else
          [t("bot.#{interval}") + " (#{t('subscriptions.upgrade')})", interval, { disabled: true }]
        end
      end
    end
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

  def render_api_key_instructions(bot)
    exchange_key = bot.exchange.name_id
    exchange_name = bot.exchange.name
    exchange_ip = bot.exchange.proxy_ip || ''
    instructions = t("bot.api.#{exchange_key}.instructions")
    content_tag(:ol, class: 'set__list') do
      instructions.map { |instruction| render_instruction(instruction, exchange_name, exchange_ip) }.join.html_safe
    end
  end

  private

  def render_instruction(instruction, exchange_name, exchange_ip, level = 1)
    text = instruction[:text_html].gsub('%{exchange_link}', exchange_name).gsub('%{ip}', exchange_ip).html_safe # rubocop:disable Style/FormatStringToken
    sub_instructions = instruction[:sub_instructions]

    content_tag(:li) do
      safe_join([
        text,
        if sub_instructions&.any?
          content_tag(level == 1 ? :ol : :ul) do
            sub_instructions.map do |sub_instruction|
              render_instruction(sub_instruction, exchange_name, exchange_ip, level + 1)
            end.join.html_safe
          end
        end
      ].compact)
    end
  end
end
