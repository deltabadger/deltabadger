module BotHelper
  def bot_intervals_select_options
    Automation::Schedulable::INTERVALS.keys.map { |interval| [t("bot.#{interval}"), interval] }
  end

  # Per-exchange label for the API key field, with a generic translated fallback.
  # Each exchange MAY define its own `bot.api.<exchange>.public_key` /
  # `private_key`; when it doesn't, we use the localized generic label under
  # `bot.api.public_key_label` / `private_key_label`.
  def api_key_field_label(exchange, field)
    specific = "bot.api.#{exchange.name_id}.#{field}"
    generic  = "bot.api.#{field}_label"
    I18n.exists?(specific) ? t(specific) : t(generic)
  end

  # One-line summary for a BotActivityLog row in the activity feed. Uses the stored
  # message when present, otherwise a translated label for the event (with light
  # detail formatting for the few high-value events).
  def bot_activity_summary(activity)
    return activity.message if activity.message.present?

    case activity.event
    when 'market_closed'
      t('bot_activity.events.market_closed', time: format_activity_time(activity.details['next_market_open_at']))
    when 'limit_paused'
      t('bot_activity.events.limit_paused', limit: activity.details['limit_type'].to_s.tr('_', ' '))
    when 'execution_failed'
      error = activity.details['error']
      if error.present?
        t('bot_activity.events.execution_failed_with_error', error: error)
      else
        t('bot_activity.events.execution_failed')
      end
    when 'order_abandoned'
      t('bot_activity.events.order_abandoned', order_id: activity.details['order_id'])
    else
      t("bot_activity.events.#{activity.event}")
    end
  end

  # Maps a transaction to a UI filter tab ('waiting' | 'cancelled' | 'successful' | nil).
  # nil means the row doesn't belong to any of the named tabs (e.g. failed/skipped).
  def order_filter_type(order)
    return 'waiting' if order.submitted? && (order.open? || order.unknown?)
    return 'cancelled' if order.submitted? && (order.cancelled? || order.abandoned?)
    return 'successful' if order.submitted? && order.closed?

    nil
  end

  # Whether an order row should render with the dimmed/inactive style. Skipped
  # rows are inactive at the status level; cancelled/abandoned at the external
  # status level.
  def inactive_order_row?(order)
    order.skipped? || (order.submitted? && (order.cancelled? || order.abandoned?))
  end

  # Human sentence for a transaction in the unified "All" timeline (the Transactions
  # tab keeps the columnar amount/value layout instead).
  def transaction_summary(order, decimals = {})
    return transaction_failed_summary(order, decimals) if order.failed?
    return t('bot_activity.transactions.skipped') if order.skipped?
    return t('bot_activity.transactions.cancelled') if order.cancelled? || order.abandoned?

    pending = order.open? || order.unknown?
    base_amount = round_amount(display_amount(order.amount_exec, order.amount, pending:), decimals[order.base])
    quote_amount = round_amount(display_amount(order.quote_amount_exec, order.quote_amount, pending:), decimals[order.quote])
    key = if order.sell?
            pending ? 'open_sell' : 'sold'
          else
            pending ? 'open_buy' : 'bought'
          end
    t("bot_activity.transactions.#{key}", amount: base_amount, base: order.base, quote_amount: quote_amount, quote: order.quote)
  end

  # While an order is still open nothing has executed yet (exec amounts are 0, not nil),
  # so show the requested amount; once it's done show what actually executed, falling
  # back to the requested amount only when no execution was recorded.
  def display_amount(executed, requested, pending:)
    return requested if pending

    executed.to_d.positive? ? executed : requested
  end

  def bot_type_label(bot)
    {
      'Bots::DcaDualAsset' => 'Rebalanced DCA',
      'Bots::DcaSingleAsset' => 'Basic DCA'
    }[bot.type]
  end

  def price_limit_value_condition_select_options(bot)
    return [] unless defined?(bot.class::PRICE_LIMIT_VALUE_CONDITIONS)

    active_timing = bot.public_send("#{trigger_prefix(bot, 'price_limit')}_timing_condition")
    bot.class::PRICE_LIMIT_VALUE_CONDITIONS.map do |condition|
      next if condition == 'between' && active_timing != 'while'

      [t("bot.settings.extra_price_limit.value_condition.#{condition}"), condition]
    end.compact
  end

  # The active side's settings-key prefix for a trigger ("price_limit" / "sell_price_limit").
  # selling? is false for non-reversible bot types, so they always read the buy-side keys.
  def trigger_prefix(bot, base)
    bot.selling? ? "sell_#{base}" : base
  end

  # The merged trigger "mode" select (issues #1/#2) — one direction-aware dropdown replacing the old
  # separate action + timing dropdowns. `base` is the unprefixed trigger name ("price_limit", …).
  #   restrict -> "Buy only"/"Sell only"   start -> "Start buying"/"Start selling"
  #   flip     -> "Start selling"/"Start buying"   (only for reversible bots)
  # Price-drop has no timing field and its pause latches, so it drops `restrict` (its non-flip mode is
  # semantically a "start").
  def trigger_mode_select_options(bot, base)
    side = bot.selling? ? 'selling' : 'buying'
    tokens = base == 'price_drop_limit' ? %w[start] : %w[restrict start]
    tokens << 'flip' if bot.reversible?
    tokens.map { |token| [t("bot.settings.trigger_mode.#{token}_#{side}"), token] }
  end

  # The mode token currently stored for the active side: flip action -> "flip"; price-drop (no
  # timing) -> "start"; otherwise the timing maps while -> "restrict", after -> "start".
  def trigger_mode_for(bot, base)
    prefix = trigger_prefix(bot, base)
    return 'flip' if %w[start_selling start_buying].include?(bot.public_send("#{prefix}_action"))
    return 'start' if base == 'price_drop_limit'

    bot.public_send("#{prefix}_timing_condition") == 'while' ? 'restrict' : 'start'
  end

  # Buying references a window high (all-time / 24h high); selling references a recent low (24h / 7d
  # low) — ATL is dropped (degenerate for selling). Labels live under separate buy/sell namespaces.
  def price_drop_limit_time_window_condition_select_options(bot)
    return [] unless defined?(bot.class::PRICE_DROP_LIMIT_BUY_TIME_WINDOW_CONDITIONS)

    conditions = bot.selling? ? bot.class::PRICE_DROP_LIMIT_SELL_TIME_WINDOW_CONDITIONS : bot.class::PRICE_DROP_LIMIT_BUY_TIME_WINDOW_CONDITIONS
    namespace = bot.selling? ? 'sell_time_window_condition' : 'time_window_condition'
    conditions.keys.map do |condition|
      [t("bot.settings.extra_price_drop_limit.#{namespace}.#{condition}"), condition]
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

  def render_api_key_instructions_for(exchange)
    render_instructions_from('bot.api', exchange)
  end

  def render_withdrawal_api_key_instructions(exchange)
    render_instructions_from('withdrawal_api', exchange)
  end

  def whitelist_ip_for(exchange)
    return nil unless exchange.present?

    proxy_url = ENV["PROXY_#{exchange.to_s.upcase}"]
    return nil unless proxy_url.present?

    URI.parse(proxy_url).host
  rescue URI::InvalidURIError
    nil
  end

  def whitelist_ip_html_for(exchange)
    ip = whitelist_ip_for(exchange)
    if ip
      "<code>#{ip}</code>"
    else
      t('bot.api.whitelist_ip_fallback_html')
    end
  end

  private

  def round_amount(value, decimals)
    return value if value.nil? || decimals.nil?

    value.round(decimals)
  end

  # Failed orders include the attempted amounts when known (so you can see what
  # failed); otherwise (e.g. a price-fetch failure) fall back to a plain message.
  def transaction_failed_summary(order, decimals)
    error = order.error_messages.to_sentence
    base_amount = round_amount(order.amount, decimals[order.base])
    quote_amount = round_amount(order.quote_amount, decimals[order.quote])

    if base_amount.present? || quote_amount.present?
      key = order.sell? ? 'failed_sell' : 'failed_buy'
      summary = t("bot_activity.transactions.#{key}", amount: base_amount, base: order.base,
                                                      quote_amount: quote_amount, quote: order.quote)
      error.present? ? "#{summary}: #{error}" : summary
    elsif error.present?
      t('bot_activity.transactions.failed_with_error', error: error)
    else
      t('bot_activity.transactions.failed')
    end
  end

  def format_activity_time(value)
    return value if value.blank?

    Time.iso8601(value.to_s).in_time_zone(current_user.time_zone).strftime('%Y-%m-%d %I:%M %p')
  rescue ArgumentError
    value
  end

  def render_instructions_from(locale_prefix, exchange)
    exchange_key = exchange.name_id
    exchange_name = exchange.name
    whitelist_ip = whitelist_ip_html_for(exchange_key)
    instructions_key = "#{locale_prefix}.#{exchange_key}.instructions"
    return nil unless I18n.exists?(instructions_key)

    instructions = t(instructions_key)
    return nil unless instructions.is_a?(Array)

    content_tag(:ol, class: 'set__list') do
      instructions.map { |instruction| render_instruction(instruction, exchange_name, whitelist_ip) }.join.html_safe
    end
  end

  def render_instruction(instruction, exchange_name, whitelist_ip = nil, level = 1)
    text = instruction[:text_html]
           .gsub('%{exchange_link}', exchange_name)
           .gsub('%{whitelist_ip}', whitelist_ip.to_s)
           .html_safe
    sub_instructions = instruction[:sub_instructions]

    content_tag(:li) do
      safe_join([
        text,
        if sub_instructions&.any?
          content_tag(level == 1 ? :ol : :ul) do
            sub_instructions.map do |sub_instruction|
              render_instruction(sub_instruction, exchange_name, whitelist_ip, level + 1)
            end.join.html_safe
          end
        end
      ].compact)
    end
  end
end
