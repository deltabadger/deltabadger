module Bot::IndicatorLimitable
  extend ActiveSupport::Concern

  INDICATOR_LIMIT_TIMING_CONDITIONS = %w[while after].freeze
  INDICATOR_LIMIT_VALUE_CONDITIONS = %w[above below].freeze
  INDICATOR_LIMIT_INDICATORS = %w[rsi].freeze
  INDICATOR_LIMIT_TIMEFRAMES = {
    'one_hour' => 1.hour,
    'four_hours' => 4.hours,
    'one_day' => 1.day,
    'three_days' => 3.days,
    'one_week' => 1.week,
    'one_month' => 1.month
  }.freeze
  # Per-side trigger action. "pause" keeps today's gate behaviour; the flip action turns the
  # trigger into a direction switch (a buy-side trigger starts selling, a sell-side one starts
  # buying), making the bot a simple trading bot.
  INDICATOR_LIMIT_BUY_ACTIONS = %w[pause start_selling].freeze
  INDICATOR_LIMIT_SELL_ACTIONS = %w[pause start_buying].freeze
  INDICATOR_LIMIT_FLIP_ACTIONS = %w[start_selling start_buying].freeze

  included do
    store_accessor :settings,
                   :indicator_limited,
                   :indicator_limit,
                   :indicator_limit_timing_condition,
                   :indicator_limit_value_condition,
                   :indicator_limit_in_ticker_id,
                   :indicator_limit_in_indicator,
                   :indicator_limit_in_timeframe,
                   :indicator_limit_action,
                   :sell_indicator_limited,
                   :sell_indicator_limit,
                   :sell_indicator_limit_timing_condition,
                   :sell_indicator_limit_value_condition,
                   :sell_indicator_limit_in_ticker_id,
                   :sell_indicator_limit_in_indicator,
                   :sell_indicator_limit_in_timeframe,
                   :sell_indicator_limit_action
    store_accessor :transient_data,
                   :indicator_limit_enabled_at,
                   :indicator_limit_condition_met_at,
                   :sell_indicator_limit_enabled_at,
                   :sell_indicator_limit_condition_met_at

    after_initialize :initialize_indicator_limitable_settings

    before_save :set_indicator_limit_enabled_at, if: :will_save_change_to_settings?
    before_save :set_indicator_limit_condition_met_at, if: :will_save_change_to_settings?
    before_save :set_indicator_limit_in_ticker_id, if: :will_save_change_to_exchange_id?
    before_save :reset_indicator_limit_info_cache, if: :will_save_change_to_settings?

    validates :indicator_limited, inclusion: { in: [true, false] }
    validates :indicator_limit, numericality: {
      message: lambda { |b, _|
                 I18n.t('activerecord.errors.models.bot.attributes.indicator_limit.not_a_number',
                        indicator: b.indicator_limit_in_indicator.upcase)
               }
    }, if: :indicator_limited?
    validates :indicator_limit_timing_condition, inclusion: { in: INDICATOR_LIMIT_TIMING_CONDITIONS }
    validates :indicator_limit_value_condition, inclusion: { in: INDICATOR_LIMIT_VALUE_CONDITIONS }
    validates :indicator_limit_in_indicator, inclusion: { in: INDICATOR_LIMIT_INDICATORS }
    validates :indicator_limit_in_timeframe, inclusion: { in: INDICATOR_LIMIT_TIMEFRAMES.keys }
    validates :indicator_limit_action, inclusion: { in: INDICATOR_LIMIT_BUY_ACTIONS }
    validates :sell_indicator_limited, inclusion: { in: [true, false] }
    validates :sell_indicator_limit, numericality: {
      message: lambda { |b, _|
                 I18n.t('activerecord.errors.models.bot.attributes.indicator_limit.not_a_number',
                        indicator: b.sell_indicator_limit_in_indicator.upcase)
               }
    }, if: :sell_indicator_limited?
    validates :sell_indicator_limit_timing_condition, inclusion: { in: INDICATOR_LIMIT_TIMING_CONDITIONS }
    validates :sell_indicator_limit_value_condition, inclusion: { in: INDICATOR_LIMIT_VALUE_CONDITIONS }
    validates :sell_indicator_limit_in_indicator, inclusion: { in: INDICATOR_LIMIT_INDICATORS }
    validates :sell_indicator_limit_in_timeframe, inclusion: { in: INDICATOR_LIMIT_TIMEFRAMES.keys }
    validates :sell_indicator_limit_action, inclusion: { in: INDICATOR_LIMIT_SELL_ACTIONS }

    decorators = Module.new do
      def parse_params(params)
        # timing_condition + action come from the merged …_mode select via expand_trigger_mode.
        super(params).merge(
          indicator_limited: params[:indicator_limited].presence&.in?(%w[1 true]),
          indicator_limit: params[:indicator_limit].presence&.to_f,
          indicator_limit_value_condition: params[:indicator_limit_value_condition].presence,
          indicator_limit_in_ticker_id: params[:indicator_limit_in_ticker_id].presence&.to_i,
          indicator_limit_in_indicator: params[:indicator_limit_in_indicator].presence,
          indicator_limit_in_timeframe: params[:indicator_limit_in_timeframe].presence,
          sell_indicator_limited: params[:sell_indicator_limited].presence&.in?(%w[1 true]),
          sell_indicator_limit: params[:sell_indicator_limit].presence&.to_f,
          sell_indicator_limit_value_condition: params[:sell_indicator_limit_value_condition].presence,
          sell_indicator_limit_in_ticker_id: params[:sell_indicator_limit_in_ticker_id].presence&.to_i,
          sell_indicator_limit_in_indicator: params[:sell_indicator_limit_in_indicator].presence,
          sell_indicator_limit_in_timeframe: params[:sell_indicator_limit_in_timeframe].presence
        ).compact.merge(expand_trigger_mode(params, 'indicator_limit', has_timing: true))
      end

      def execute_action
        return super unless active_indicator_limited?

        met = indicator_limit_condition_currently_met?
        if active_indicator_limit_flip?
          # A flip trigger only watches; it never pauses trading. When met, flip and break the
          # reschedule (no super → at most one flip per run; the fresh ActionJob runs the new side).
          return super unless met

          flip_direction!
          Result::Success.new({ break_reschedule: true })
        elsif met
          super
        else
          update!(status: :waiting)
          log_activity('limit_paused', details: { limit_type: :indicator })
          next_check_at = Time.now.utc + Utilities::Time.seconds_to_current_candle_close(indicator_limit_in_timeframe_duration)
          Bot::IndicatorLimitCheckJob.set(wait_until: next_check_at).perform_later(self)
          Result::Success.new({ break_reschedule: true })
        end
      end

      def stop(stop_message_key: nil)
        is_stopped = super(stop_message_key:)
        return is_stopped unless indicator_limited? || sell_indicator_limited?

        cancel_scheduled_indicator_limit_check_jobs
        is_stopped
      end

      def started_at
        return super unless active_indicator_limited?

        condition_met_at = active_indicator_limit_condition_met_at
        if super.nil? || condition_met_at.nil?
          nil
        else
          [super, condition_met_at].max
        end
      end
    end

    prepend decorators
  end

  def indicator_limit_enabled_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def indicator_limit_condition_met_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def sell_indicator_limit_enabled_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def sell_indicator_limit_condition_met_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def indicator_limit_in_timeframe_duration
    INDICATOR_LIMIT_TIMEFRAMES[active_indicator_limit_in_timeframe]
  end

  # Action reader fallbacks (default "pause", never persisted-on-load per invariant 1).
  def indicator_limit_action
    super.presence || 'pause'
  end

  def sell_indicator_limit_action
    super.presence || 'pause'
  end

  # Sell-side read-time fallbacks (never persisted-on-load — see price_limitable for the rationale).
  {
    sell_indicator_limited: false,
    sell_indicator_limit: 70,
    sell_indicator_limit_timing_condition: 'while',
    sell_indicator_limit_value_condition: 'above',
    sell_indicator_limit_in_indicator: 'rsi',
    sell_indicator_limit_in_timeframe: 'one_day'
  }.each do |name, default|
    define_method(name) do
      value = super()
      value.nil? ? default : value
    end
  end

  def sell_indicator_limit_in_ticker_id
    super.presence || tickers.min_by { |t| t[:base] }&.id
  end

  def indicator_limited?
    indicator_limited == true
  end

  def sell_indicator_limited?
    sell_indicator_limited == true
  end

  # Sell-side reader fallbacks for indicator and timeframe (mirror buy defaults).
  def sell_indicator_limit_in_indicator
    super.presence || 'rsi'
  end

  def sell_indicator_limit_in_timeframe
    super.presence || 'one_day'
  end

  # The active side's view of this trigger (picked by direction). Decorators read these.
  def active_indicator_limited?
    selling? ? sell_indicator_limited? : indicator_limited?
  end

  def active_indicator_limit_action
    selling? ? sell_indicator_limit_action : indicator_limit_action
  end

  def active_indicator_limit_flip?
    reversible? && INDICATOR_LIMIT_FLIP_ACTIONS.include?(active_indicator_limit_action)
  end

  def active_indicator_limit_condition_met_at
    selling? ? sell_indicator_limit_condition_met_at : indicator_limit_condition_met_at
  end

  def active_indicator_limit_in_timeframe
    public_send("#{indicator_limit_prefix}_in_timeframe")
  end

  # Evaluate the ACTIVE side's indicator condition, writing that side's condition_met_at. The check
  # job (Bot::IndicatorLimitCheckJob) polls this method unchanged for either direction.
  def get_indicator_limit_condition_met?
    return Result::Success.new(false) unless active_indicator_limited?
    return Result::Success.new(true) if timing_condition_satisfied?

    ticker = tickers.available.find_by(id: public_send("#{indicator_limit_prefix}_in_ticker_id"))
    return Result::Success.new(false) unless ticker.present?

    result = get_indicator_value(ticker)
    return result if result.failure?

    if indicator_condition_satisfied?(result.data)
      if active_indicator_limit_condition_met_at.nil?
        update!("#{indicator_limit_prefix}_condition_met_at" => Time.current)
        broadcast_indicator_limit_info_update
      end
      Result::Success.new(true)
    else
      if active_indicator_limit_condition_met_at.present?
        set_missed_quote_amount
        update!("#{indicator_limit_prefix}_condition_met_at" => nil)
        broadcast_indicator_limit_info_update
      end
      Result::Success.new(false)
    end
  end

  def broadcast_indicator_limit_info_update
    ticker = tickers.available.find_by(id: public_send("#{indicator_limit_prefix}_in_ticker_id"))
    return unless ticker.present?

    indicator_value_result = get_indicator_value(ticker)
    return if indicator_value_result.failure?
    return unless indicator_value_result.data.present?

    condition_met_result = get_indicator_limit_condition_met?
    return if condition_met_result.failure?

    info = Rails.cache.fetch(indicator_limit_info_cache_key, expires_in: 20.seconds) do
      {
        value: indicator_value_result.data,
        condition_met: condition_met_result.data
      }
    end

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: new_record? ? 'new-settings-indicator-limit-info' : 'settings-indicator-limit-info',
      partial: 'bots/settings/indicator_limit_info',
      locals: { bot: self, info: }
    )
  end

  def indicator_limit_info_from_cache
    Rails.cache.read(indicator_limit_info_cache_key)
  end

  private

  # "indicator_limit" while buying, "sell_indicator_limit" while selling — the key prefix for
  # the active side's mirror. Used by the read/eval path (not the per-side persistence callbacks).
  def indicator_limit_prefix
    selling? ? 'sell_indicator_limit' : 'indicator_limit'
  end

  def indicator_limit_condition_currently_met?
    result = get_indicator_limit_condition_met?
    result.success? && result.data
  end

  # Side-suffixed so a flip never renders the buy-side reading from a stale cache entry.
  def indicator_limit_info_cache_key
    "bot_#{id}_indicator_limit_info_#{selling? ? 'selling' : 'buying'}"
  end

  def reset_indicator_limit_info_cache
    return if indicator_limited_was == indicator_limited &&
              indicator_limit_value_condition_was == indicator_limit_value_condition &&
              indicator_limit_in_ticker_id_was == indicator_limit_in_ticker_id &&
              indicator_limit_in_indicator_was == indicator_limit_in_indicator &&
              indicator_limit_in_timeframe_was == indicator_limit_in_timeframe &&
              sell_indicator_limited_was == sell_indicator_limited &&
              sell_indicator_limit_value_condition_was == sell_indicator_limit_value_condition &&
              sell_indicator_limit_in_ticker_id_was == sell_indicator_limit_in_ticker_id &&
              sell_indicator_limit_in_indicator_was == sell_indicator_limit_in_indicator &&
              sell_indicator_limit_in_timeframe_was == sell_indicator_limit_in_timeframe

    Rails.cache.delete(indicator_limit_info_cache_key)
  end

  def timing_condition_satisfied?
    public_send("#{indicator_limit_prefix}_timing_condition") == 'after' &&
      active_indicator_limit_condition_met_at.present?
  end

  def indicator_condition_satisfied?(value)
    case public_send("#{indicator_limit_prefix}_value_condition")
    when 'below'
      value < public_send(indicator_limit_prefix)
    when 'above'
      value > public_send(indicator_limit_prefix)
    else
      false
    end
  end

  def get_indicator_value(ticker)
    case public_send("#{indicator_limit_prefix}_in_indicator")
    when 'rsi'
      ticker.get_rsi_value(
        timeframe: INDICATOR_LIMIT_TIMEFRAMES[public_send("#{indicator_limit_prefix}_in_timeframe")],
        period: 14
      )
    else
      raise "Unknown indicator: #{public_send("#{indicator_limit_prefix}_in_indicator")}"
    end
  end

  def initialize_indicator_limitable_settings
    self.indicator_limited ||= false
    self.indicator_limit ||= 30
    self.indicator_limit_timing_condition ||= 'while'
    self.indicator_limit_value_condition ||= 'below'
    self.indicator_limit_in_ticker_id ||= tickers.min_by { |t| t[:base] }&.id
    self.indicator_limit_in_indicator ||= 'rsi'
    self.indicator_limit_in_timeframe ||= 'one_day'
    # Sell-side defaults are read-time fallbacks (see readers above), never written on load.
  end

  def set_indicator_limit_enabled_at
    if indicator_limited_was != indicator_limited
      self.indicator_limit_enabled_at = indicator_limited? ? Time.current : nil
    end
    return if sell_indicator_limited_was == sell_indicator_limited

    self.sell_indicator_limit_enabled_at = sell_indicator_limited? ? Time.current : nil
  end

  def set_indicator_limit_condition_met_at
    self.indicator_limit_condition_met_at = nil if indicator_limited_was != indicator_limited
    self.sell_indicator_limit_condition_met_at = nil if sell_indicator_limited_was != sell_indicator_limited
  end

  def set_indicator_limit_in_ticker_id
    if indicator_limit_in_ticker_id_was.present? && exchange_id_was.present? && exchange_id_was != exchange_id
      ticker_was = Ticker.find_by(id: indicator_limit_in_ticker_id_was)
      self.indicator_limit_in_ticker_id = tickers.find_by(
        base_asset_id: ticker_was.base_asset_id,
        quote_asset_id: ticker_was.quote_asset_id
      )&.id
      sell_ticker_was = Ticker.find_by(id: sell_indicator_limit_in_ticker_id_was) if sell_indicator_limit_in_ticker_id_was.present?
      self.sell_indicator_limit_in_ticker_id = if sell_ticker_was
                                                 tickers.find_by(base_asset_id: sell_ticker_was.base_asset_id,
                                                                 quote_asset_id: sell_ticker_was.quote_asset_id)&.id
                                               else
                                                 tickers.min_by { |t| t[:base] }&.id
                                               end
    else
      default_ticker_id = tickers.min_by { |t| t[:base] }&.id
      self.indicator_limit_in_ticker_id = default_ticker_id
      self.sell_indicator_limit_in_ticker_id = default_ticker_id
    end
  end

  def cancel_scheduled_indicator_limit_check_jobs
    cancel_solid_queue_jobs(
      job_class: 'Bot::IndicatorLimitCheckJob',
      record: self
    )
  end
end
