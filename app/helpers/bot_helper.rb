module BotHelper
  def bot_intervals_select_options
    Automation::Schedulable::INTERVALS.keys.map { |interval| [t("bot.#{interval}"), interval] }
  end

  # The PnL curve of the bot chart: how far ahead of what was put in, at every point, in the
  # quote currency.
  #
  # The VALUE curve always climbs for a DCA bot, because the money going in climbs — its shape
  # is deposits, not performance. This makes the invested line the zero line, so the curve is
  # the distance from it and the shape is performance alone.
  #
  # ABSOLUTE, not a percentage of what has been invested so far. A deposit adds the same amount
  # to both terms, so it moves this curve by nothing — while as a ratio it moves the
  # DENOMINATOR: buying 100 into a 110/100 position gives 210/200, and a flat market would draw
  # a drop from +10% to +5%. That is the deposit sawtooth this mode exists to remove, so the
  # curve cannot be the one that has it.
  #
  # Selling needs no special case: proceeds stay inside `value` as realized cash while
  # `invested` holds its level, so a locked-in gain goes on reading as a gain.
  def chart_pnl_series(values, invested)
    # Subtracted in decimal: production metrics are BigDecimal, and going through Float first
    # would turn 100.005 - 100.00 into 0.004999…, which rounds to 0.00 on a two-decimal quote
    # while the headline — rounded from the same source series — says +0.01.
    values.each_with_index.map { |value, i| value.to_d - (invested[i] || 0).to_d }
  end

  # == the dashboard's mini P/L curve ==
  #
  # The zero line is the bottom rule of `.dash-intro`, which is why the geometry is written in a
  # 100-unit box ABOVE it: y=100 is that rule, y=0 is the top of the headline. Below the rule is
  # the same box mirrored — a fixed 100 units, not however deep this account's worst day happens
  # to be, so the page under the chart sits where it sits whatever the curve does.
  #
  # The plot itself is drawn the way the bot and tracker charts draw theirs: one smoothed curve,
  # green above the line and red below it, filled to the line, with a dot on the last point.
  # Splitting the colours is left to two clip paths in the view, so the crossing is exact rather
  # than resolved per segment.

  # Ten percent to reach an edge. Without a floor the scale is whatever the account happened to do,
  # so a flat fortnight would be redrawn as a mountain range and touching the top would mean
  # nothing at all.
  SPARK_MIN_SCALE = 0.10
  # And thirty days to earn the full width. A week-old account gets a short curve rather than one
  # stretched out of nothing.
  SPARK_FULL_DAYS = 30

  # The headline's two formats, in one place: the view writes them and pnl_spark_controller.js
  # rebuilds them under the pointer. Two readings of one figure that have to agree character for
  # character, or the number changes shape as it is hovered.
  def pnl_headline_percent(value)
    "#{'+' if value.positive?}#{format_percent(value, precision: 2)}"
  end

  def pnl_headline_amount(usd, denomination)
    safe_join([('+' if usd.positive?), denomination.format(usd, precision: 0)].compact)
  end

  def pnl_spark(history)
    percent = history[:percent]
    scale = [percent.map(&:abs).max, SPARK_MIN_SCALE].max
    points = percent.each_with_index.map do |value, i|
      [i * (100.0 / (percent.size - 1)), (1 - (value / scale)) * 100]
    end
    curve = spark_curve(points)

    {
      path: curve,
      # Closed along the zero line, so the ribbon is the distance from it — the same thing the
      # curve is. One shape for both colours; the view clips it into halves.
      area: "#{curve} L100,100 L0,100 Z",
      scale: scale,
      # Where the last point sits in the whole 200-unit box, for the dot that marks it — a
      # percentage, because that box is only ever measured in rems by the CSS.
      end_y: (points.last[1] / 2).round(3),
      width: ([history[:days] / SPARK_FULL_DAYS.to_f, 1].min * 100).round(2),
      gain: !percent.last.negative?
    }
  end

  private

  # Monotone cubic interpolation (Fritsch–Carlson), which is what `cubicInterpolationMode:
  # "monotone"` gives the charts this curve is meant to match. Monotone and not a plain spline
  # because it cannot overshoot the data: the box is sized to the extremes, so a curve that
  # bulged past them would be drawn straight into the clip.
  def spark_curve(points)
    slopes = spark_slopes(points)
    segments = points.each_cons(2).with_index.map do |(from, to), i|
      third = (to[0] - from[0]) / 3.0
      format('C%g,%g %g,%g %g,%g',
             (from[0] + third).round(2), (from[1] + (slopes[i] * third)).round(2),
             (to[0] - third).round(2), (to[1] - (slopes[i + 1] * third)).round(2),
             to[0].round(2), to[1].round(2))
    end

    "M#{format('%g,%g', points[0][0].round(2), points[0][1].round(2))} #{segments.join(' ')}"
  end

  # The tangent at each point: the average of its neighbours' secants, then pulled back wherever
  # that would turn a rise into a dip.
  def spark_slopes(points)
    secants = points.each_cons(2).map { |from, to| (to[1] - from[1]) / (to[0] - from[0]) }
    slopes = [secants.first] + secants.each_cons(2).map { |before, after| (before + after) / 2.0 } + [secants.last]

    secants.each_with_index do |secant, i|
      if secant.zero?
        slopes[i] = slopes[i + 1] = 0.0
        next
      end

      alpha = slopes[i] / secant
      beta = slopes[i + 1] / secant
      slopes[i] = 0.0 if alpha.negative?
      slopes[i + 1] = 0.0 if beta.negative?
      next unless (alpha**2) + (beta**2) > 9

      tau = 3.0 / Math.sqrt((alpha**2) + (beta**2))
      slopes[i] = tau * alpha * secant
      slopes[i + 1] = tau * beta * secant
    end
    slopes
  end

  public

  # Per-exchange label for the API key field, with a generic translated fallback.
  # Each exchange MAY define its own `bot.api.<exchange>.public_key` /
  # `private_key`; when it doesn't, we use the localized generic label under
  # `bot.api.public_key_label` / `private_key_label`.
  def api_key_field_label(exchange, field)
    specific = "bot.api.#{exchange.name_id}.#{field}"
    generic  = "bot.api.#{field}_label"
    I18n.exists?(specific) ? t(specific) : t(generic)
  end

  # The one question every "hide balances" branch asks.
  #
  # Asked of the BOT, not of current_user: most of the partials that ask it are also rendered by a
  # turbo broadcast from a background job, where there is no request and no current_user.
  #
  # persisted? folds in the creation wizard, which renders the same settings partials with a
  # new_record bot — there the amount field IS the question being asked, so it is never hidden.
  def hide_balances?(bot)
    bot.persisted? && bot.user.hide_balances?
  end

  # One-line summary for a BotActivityLog row in the activity feed. Uses the stored
  # message when present, otherwise a translated label for the event (with light
  # detail formatting for the few high-value events).
  # A money figure that rounds to nothing, dimmed.
  #
  # 0.00 sitting in the same weight as a real balance reads as a number worth comparing, and on a
  # holdings table most of them are not — an average price nobody paid, a value too small to have
  # one. Dimmed rather than blanked: the row still has to state what it holds.
  # A figure with the currency it is actually in beside it, the CODE in <small>: USDT is what
  # the bot spends, and a code is how a transaction currency is written. The symbol is reserved
  # for the reader's own chosen denomination (Denomination#format), which this is not.
  def quoted_figure(figure, quote)
    return figure if figure.blank? || quote.blank?

    safe_join([figure, ' ', tag.small(quote)])
  end

  def money_figure(value, precision: 2, **options)
    formatted = number_with_precision(value || 0, precision: precision, delimiter: ',', **options)
    return formatted unless value.to_d.round(precision).zero?

    tag.span(formatted, class: 'is-zero')
  end

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
    when 'liquidation_failed'
      t('bot_activity.events.liquidation_failed', error: activity.details['reason'])
    when 'redeploy_failed'
      t('bot_activity.events.redeploy_failed', error: activity.details['reason'])
    when 'order_abandoned'
      t('bot_activity.events.order_abandoned', order_id: activity.details['order_id'])
    else
      t("bot_activity.events.#{activity.event}")
    end
  end

  # A money amount inside a settings sentence — the contribution, a sell size, a spend cap, a
  # smart-interval slice. While balances are hidden the field is still IN the form and still POSTS
  # its value; it just isn't shown, which is what turns "Invest 100 USD / week" into "Invest USD /
  # week" — the ticker beside it never moves.
  #
  # Hidden rather than dropped, because these forms submit as a whole: changing the interval or
  # dragging an allocation posts every field, and one that had vanished would blank the bot's
  # contribution. A price keeps its number and never comes through here: a market level is a fact
  # about the venue, not about the holder.
  def amount_field(form, name, **options)
    return form.number_field(name, **options) unless hide_balances?(form.object)

    form.hidden_field(name, value: options[:value])
  end

  # The log's filter tabs, in display order: [value, label, available?].
  #
  # "All" is the sentence timeline — "Bought 0.00023 BTC for 100 USD" — the one tab that spells the
  # money out in prose, so hiding balances drops it. The activity rows that lived only there move to
  # Other, which this code already calls the catch-all for what the named tabs don't want; counted
  # through the FEED's own exclusions, or an order_skipped log on its own would open an Other tab
  # with nothing in it.
  def order_filter_tabs(bot)
    hidden = hide_balances?(bot)
    activities = hidden && bot.bot_activity_logs.where.not(event: BotActivityFeed::EXCLUDED_EVENTS).exists?

    [(['all', t('order_filters.all'), true] unless hidden),
     ['successful', t('order_filters.transactions'), bot.transactions.submitted.closed.exists?],
     ['waiting', t('order_filters.waiting'), bot.transactions.waiting.exists?],
     ['other', t('order_filters.other'), bot.transactions.other.exists? || activities]].compact
  end

  # The tab the log opens on: "All" normally, the first tab that has rows while hiding. Falls back
  # to a real tab name rather than nil so a bot with no rows at all still names something — the
  # only row on screen then is the placeholder, which belongs to no tab and is always visible.
  def default_order_filter(tabs)
    tabs.find(&:last)&.first || 'successful'
  end

  # The tab a COLUMNAR row belongs to ('waiting' | 'successful' | nil). nil is every
  # Transaction#other? row — cancelled, abandoned, skipped, failed: they have no
  # columnar home and show under "Other" as their sentence row, so the columnar row,
  # where one was rendered at all, belongs to no tab and stays hidden.
  def order_filter_type(order)
    return 'waiting' if order.submitted? && (order.open? || order.unknown?)
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
    base_amount = round_amount(display_amount(order.amount_exec, order.amount, pending:), decimals[order.base], order.base)
    quote_amount = round_amount(display_amount(order.quote_amount_exec, order.quote_amount, pending:), decimals[order.quote], order.quote)
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

  # The assets a trading condition may watch. For a composition bot that is its current members —
  # NOT bot.tickers, which deliberately spans the venue's whole quote catalogue so that removed
  # holdings keep pricing. Offering one of those would let a condition watch an asset the bot no
  # longer holds, and there can be thousands of them.
  def base_select_options(bot)
    tickers = bot.respond_to?(:composition_tickers) ? bot.composition_tickers : bot.tickers
    tickers.map { |ticker| [ticker.base_asset.symbol, ticker.id] }.sort_by(&:first)
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

  # A reading key's list where a venue has one, and the bot's list where it does not — the same walk
  # with one checkbox more than the tracker needs, which beats no instructions at all.
  def render_read_api_key_instructions(exchange)
    render_instructions_from('read_only_api', exchange) || render_api_key_instructions_for(exchange)
  end

  def render_api_key_instructions(api_key)
    case api_key.key_type
    when 'withdrawal' then render_withdrawal_api_key_instructions(api_key.exchange)
    when 'read_only' then render_read_api_key_instructions(api_key.exchange)
    else render_api_key_instructions_for(api_key.exchange)
    end
  end

  # Whether there is anything to show — Alpaca, IBKR and retired venues have no instruction list,
  # and a link to an empty modal is worse than no link. Same I18n.exists? probe the renderer uses,
  # EN fallback included on purpose: withdrawal_api exists only in English, and an English list
  # beats hiding the link from everyone else.
  def api_key_instructions?(api_key)
    # A reading key always has a list — its own where the venue needs one, the bot's otherwise.
    prefix = api_key.withdrawal? ? 'withdrawal_api' : 'bot.api'
    I18n.exists?("#{prefix}.#{api_key.exchange.name_id}.instructions")
  end

  def whitelist_ip_for(exchange)
    return nil unless exchange.present?

    proxy_url = ExchangeProxy.for(exchange)
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

  # Money is written to the cent, whatever precision the venue happens to publish for the pair: a
  # column of dollars is read by comparing the figures in it, and 10.0 beside 9.99 beside 0.001407
  # cannot be. Anything else keeps the venue's decimals, where the difference between eight places
  # and two is the difference between a quantity and nothing at all.
  def round_amount(value, decimals, currency = nil)
    return value if value.nil?
    return number_with_precision(value, precision: 2) if Tax::PriceService.money?(currency)
    return value if decimals.nil?

    value.round(decimals)
  end

  # Failed orders include the attempted amounts when known (so you can see what
  # failed); otherwise (e.g. a price-fetch failure) fall back to a plain message.
  def transaction_failed_summary(order, decimals)
    error = order.error_messages.to_sentence
    base_amount = round_amount(order.amount, decimals[order.base], order.base)
    quote_amount = round_amount(order.quote_amount, decimals[order.quote], order.quote)

    # Failed rows are the one kind of sentence the Other tab still shows while balances are
    # hidden, and the attempted amounts are the only money in them — dropping them leaves the
    # error, which is what the row is there to report.
    if hide_balances?(order.bot)
      base_amount = nil
      quote_amount = nil
    end

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

    at = Time.iso8601(value.to_s)
    "#{table_date(at, current_user.time_zone)} #{table_clock(at, current_user.time_zone)}"
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

  # == the menu's dashboard icon ==
  #
  # The icon IS the number of live bots — the same figure the bots page counts in its headline, so
  # archived bots are out of both.
  def bot_menu_count(user) = user.bots.not_deleted.not_archived.size

  # 19 sets the digits to the height of the drawn icons beside them. Two still fit the box across;
  # a third does not, so from there on the number gives up height for width. 0.549em is Dosis's
  # digit advance at the weight `_navbar.sass` asks for, and 22 leaves the box a hair of margin.
  DIGIT_ADVANCE = 0.549
  def bot_count_font_size(count) = [19, (22 / (count.to_s.length * DIGIT_ADVANCE)).floor].min

  # Dosis's digits are 0.731em tall, so half of that below the box's centre puts them optically in
  # the middle. Written as a baseline rather than `dominant-baseline`, which Safari reads its own way.
  def bot_count_baseline(size) = (12 + (size * 0.3655)).round(2)
end
