module Bot::MovingAverageLimitable
  extend ActiveSupport::Concern

  MOVING_AVERAGE_LIMIT_TIMING_CONDITIONS = %w[while after].freeze
  MOVING_AVERAGE_LIMIT_VALUE_CONDITIONS = %w[above below].freeze
  MOVING_AVERAGE_LIMIT_MA_TYPES = %w[sma ema].freeze
  MOVING_AVERAGE_LIMIT_TIMEFRAMES = {
    'one_hour' => 1.hour,
    'four_hours' => 4.hours,
    'one_day' => 1.day,
    'three_days' => 3.days,
    'one_week' => 1.week,
    'one_month' => 1.month
  }.freeze
  MOVING_AVERAGE_LIMIT_BUY_ACTIONS = %w[pause start_selling].freeze
  MOVING_AVERAGE_LIMIT_SELL_ACTIONS = %w[pause start_buying].freeze
  MOVING_AVERAGE_LIMIT_FLIP_ACTIONS = %w[start_selling start_buying].freeze

  included do
    store_accessor :settings,
                   :moving_average_limited,
                   :moving_average_limit_timing_condition,
                   :moving_average_limit_value_condition,
                   :moving_average_limit_in_ticker_id,
                   :moving_average_limit_in_ma_type,
                   :moving_average_limit_in_timeframe,
                   :moving_average_limit_in_period,
                   :moving_average_limit_action,
                   :sell_moving_average_limited,
                   :sell_moving_average_limit_timing_condition,
                   :sell_moving_average_limit_value_condition,
                   :sell_moving_average_limit_in_ticker_id,
                   :sell_moving_average_limit_in_ma_type,
                   :sell_moving_average_limit_in_timeframe,
                   :sell_moving_average_limit_in_period,
                   :sell_moving_average_limit_action
    store_accessor :transient_data,
                   :moving_average_limit_enabled_at,
                   :moving_average_limit_condition_met_at,
                   :sell_moving_average_limit_enabled_at,
                   :sell_moving_average_limit_condition_met_at

    after_initialize :initialize_moving_average_limitable_settings

    before_save :set_moving_average_limit_enabled_at, if: :will_save_change_to_settings?
    before_save :set_moving_average_limit_condition_met_at, if: :will_save_change_to_settings?
    before_save :set_moving_average_limit_in_ticker_id, if: :will_save_change_to_exchange_id?
    before_save :reset_moving_average_limit_info_cache, if: :will_save_change_to_settings?

    validates :moving_average_limited, inclusion: { in: [true, false] }
    validates :moving_average_limit_timing_condition, inclusion: { in: MOVING_AVERAGE_LIMIT_TIMING_CONDITIONS }
    validates :moving_average_limit_value_condition, inclusion: { in: MOVING_AVERAGE_LIMIT_VALUE_CONDITIONS }
    validates :moving_average_limit_in_ma_type, inclusion: { in: MOVING_AVERAGE_LIMIT_MA_TYPES }
    validates :moving_average_limit_in_timeframe, inclusion: { in: MOVING_AVERAGE_LIMIT_TIMEFRAMES.keys }
    validates :moving_average_limit_in_period, numericality: { only_integer: true, greater_than: 0 }, if: :moving_average_limited?
    validates :moving_average_limit_action, inclusion: { in: MOVING_AVERAGE_LIMIT_BUY_ACTIONS }
    validates :sell_moving_average_limited, inclusion: { in: [true, false] }
    validates :sell_moving_average_limit_timing_condition, inclusion: { in: MOVING_AVERAGE_LIMIT_TIMING_CONDITIONS }
    validates :sell_moving_average_limit_value_condition, inclusion: { in: MOVING_AVERAGE_LIMIT_VALUE_CONDITIONS }
    validates :sell_moving_average_limit_in_ma_type, inclusion: { in: MOVING_AVERAGE_LIMIT_MA_TYPES }
    validates :sell_moving_average_limit_in_timeframe, inclusion: { in: MOVING_AVERAGE_LIMIT_TIMEFRAMES.keys }
    validates :sell_moving_average_limit_in_period, numericality: { only_integer: true, greater_than: 0 }, if: :sell_moving_average_limited?
    validates :sell_moving_average_limit_action, inclusion: { in: MOVING_AVERAGE_LIMIT_SELL_ACTIONS }

    decorators = Module.new do
      def parse_params(params)
        # timing_condition + action come from the merged …_mode select via expand_trigger_mode.
        super(params).merge(
          moving_average_limited: params[:moving_average_limited].presence&.in?(%w[1 true]),
          moving_average_limit_value_condition: params[:moving_average_limit_value_condition].presence,
          moving_average_limit_in_ticker_id: params[:moving_average_limit_in_ticker_id].presence&.to_i,
          moving_average_limit_in_ma_type: params[:moving_average_limit_in_ma_type].presence,
          moving_average_limit_in_timeframe: params[:moving_average_limit_in_timeframe].presence,
          moving_average_limit_in_period: params[:moving_average_limit_in_period].presence&.to_i,
          sell_moving_average_limited: params[:sell_moving_average_limited].presence&.in?(%w[1 true]),
          sell_moving_average_limit_value_condition: params[:sell_moving_average_limit_value_condition].presence,
          sell_moving_average_limit_in_ticker_id: params[:sell_moving_average_limit_in_ticker_id].presence&.to_i,
          sell_moving_average_limit_in_ma_type: params[:sell_moving_average_limit_in_ma_type].presence,
          sell_moving_average_limit_in_timeframe: params[:sell_moving_average_limit_in_timeframe].presence,
          sell_moving_average_limit_in_period: params[:sell_moving_average_limit_in_period].presence&.to_i
        ).compact.merge(expand_trigger_mode(params, 'moving_average_limit', has_timing: true))
      end

      def execute_action
        return super unless active_moving_average_limited?

        met = moving_average_limit_condition_currently_met?
        if active_moving_average_limit_flip?
          return super unless met

          flip_direction!
          Result::Success.new({ break_reschedule: true })
        elsif met
          super
        else
          update!(status: :waiting)
          log_activity('limit_paused', details: { limit_type: :moving_average })
          next_check_at = Time.now.utc + Utilities::Time.seconds_to_current_candle_close(moving_average_limit_in_timeframe_duration)
          Bot::MovingAverageLimitCheckJob.set(wait_until: next_check_at).perform_later(self)
          Result::Success.new({ break_reschedule: true })
        end
      end

      def stop(stop_message_key: nil)
        is_stopped = super(stop_message_key:)
        return is_stopped unless moving_average_limited? || sell_moving_average_limited?

        cancel_scheduled_moving_average_limit_check_jobs
        is_stopped
      end

      def started_at
        return super unless active_moving_average_limited?

        condition_met_at = active_moving_average_limit_condition_met_at
        if super.nil? || condition_met_at.nil?
          nil
        else
          [super, condition_met_at].max
        end
      end
    end

    prepend decorators
  end

  def moving_average_limit_enabled_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def moving_average_limit_condition_met_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def sell_moving_average_limit_enabled_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def sell_moving_average_limit_condition_met_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def moving_average_limit_in_timeframe_duration
    # Active-side timeframe so the check job's next_check_at aligns with the side actually being
    # evaluated (consistent with IndicatorLimitable); buy-side for non-reversible bots.
    MOVING_AVERAGE_LIMIT_TIMEFRAMES[public_send("#{moving_average_limit_prefix}_in_timeframe")]
  end

  # Action reader fallbacks (default "pause", never persisted-on-load per invariant 1).
  def moving_average_limit_action
    super.presence || 'pause'
  end

  def sell_moving_average_limit_action
    super.presence || 'pause'
  end

  # Sell-side read-time fallbacks (never persisted-on-load — see price_limitable for the rationale).
  {
    sell_moving_average_limited: false,
    sell_moving_average_limit_timing_condition: 'while',
    sell_moving_average_limit_value_condition: 'above',
    sell_moving_average_limit_in_ma_type: 'sma',
    sell_moving_average_limit_in_timeframe: 'one_day',
    sell_moving_average_limit_in_period: 9
  }.each do |name, default|
    define_method(name) do
      value = super()
      value.nil? ? default : value
    end
  end

  def sell_moving_average_limit_in_ticker_id
    super.presence || tickers.min_by { |t| t[:base] }&.id
  end

  def moving_average_limited?
    moving_average_limited == true
  end

  def sell_moving_average_limited?
    sell_moving_average_limited == true
  end

  # The active side's view of this trigger (picked by direction). Decorators read these.
  def active_moving_average_limited?
    selling? ? sell_moving_average_limited? : moving_average_limited?
  end

  def active_moving_average_limit_action
    selling? ? sell_moving_average_limit_action : moving_average_limit_action
  end

  def active_moving_average_limit_flip?
    reversible? && MOVING_AVERAGE_LIMIT_FLIP_ACTIONS.include?(active_moving_average_limit_action)
  end

  def active_moving_average_limit_condition_met_at
    selling? ? sell_moving_average_limit_condition_met_at : moving_average_limit_condition_met_at
  end

  # Evaluate the ACTIVE side's MA condition, writing that side's condition_met_at. The check
  # job (Bot::MovingAverageLimitCheckJob) polls this method unchanged for either direction.
  def get_moving_average_limit_condition_met?
    return Result::Success.new(false) unless active_moving_average_limited?
    return Result::Success.new(true) if timing_condition_satisfied?

    ticker = tickers.available.find_by(id: public_send("#{moving_average_limit_prefix}_in_ticker_id"))
    return Result::Success.new(false) unless ticker.present?

    price_result = ticker.get_last_price
    return price_result if price_result.failure?

    moving_average_value_result = get_moving_average_value(ticker)
    return moving_average_value_result if moving_average_value_result.failure?

    if moving_average_condition_satisfied?(price_result.data, moving_average_value_result.data)
      if active_moving_average_limit_condition_met_at.nil?
        update!("#{moving_average_limit_prefix}_condition_met_at" => Time.current)
        broadcast_moving_average_limit_info_update
      end
      Result::Success.new(true)
    else
      if active_moving_average_limit_condition_met_at.present?
        set_missed_quote_amount
        update!("#{moving_average_limit_prefix}_condition_met_at" => nil)
        broadcast_moving_average_limit_info_update
      end
      Result::Success.new(false)
    end
  end

  def broadcast_moving_average_limit_info_update
    ticker = tickers.available.find_by(id: public_send("#{moving_average_limit_prefix}_in_ticker_id"))
    return unless ticker.present?

    moving_average_value_result = get_moving_average_value(ticker)
    return if moving_average_value_result.failure?
    return unless moving_average_value_result.data.present?

    condition_met_result = get_moving_average_limit_condition_met?
    return if condition_met_result.failure?

    info = Rails.cache.fetch(moving_average_limit_info_cache_key, expires_in: 20.seconds) do
      {
        value: moving_average_value_result.data,
        condition_met: condition_met_result.data
      }
    end

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: new_record? ? 'new-settings-moving-average-limit-info' : 'settings-moving-average-limit-info',
      partial: 'bots/settings/moving_average_limit_info',
      locals: { bot: self, info: }
    )
  end

  def moving_average_limit_info_from_cache
    Rails.cache.read(moving_average_limit_info_cache_key)
  end

  private

  # "moving_average_limit" while buying, "sell_moving_average_limit" while selling — the key
  # prefix for the active side's mirror. Used by the read/eval path (not per-side callbacks).
  def moving_average_limit_prefix
    selling? ? 'sell_moving_average_limit' : 'moving_average_limit'
  end

  def moving_average_limit_condition_currently_met?
    result = get_moving_average_limit_condition_met?
    result.success? && result.data
  end

  # Side-suffixed so a flip never renders the buy-side reading from a stale cache entry.
  def moving_average_limit_info_cache_key
    "bot_#{id}_moving_average_limit_info_#{selling? ? 'selling' : 'buying'}"
  end

  def reset_moving_average_limit_info_cache
    return if moving_average_limit_value_condition_was == moving_average_limit_value_condition &&
              moving_average_limit_in_ticker_id_was == moving_average_limit_in_ticker_id &&
              moving_average_limit_in_ma_type_was == moving_average_limit_in_ma_type &&
              moving_average_limit_in_timeframe_was == moving_average_limit_in_timeframe &&
              moving_average_limit_in_period_was == moving_average_limit_in_period &&
              sell_moving_average_limit_value_condition_was == sell_moving_average_limit_value_condition &&
              sell_moving_average_limit_in_ticker_id_was == sell_moving_average_limit_in_ticker_id &&
              sell_moving_average_limit_in_ma_type_was == sell_moving_average_limit_in_ma_type &&
              sell_moving_average_limit_in_timeframe_was == sell_moving_average_limit_in_timeframe &&
              sell_moving_average_limit_in_period_was == sell_moving_average_limit_in_period

    Rails.cache.delete(moving_average_limit_info_cache_key)
  end

  def timing_condition_satisfied?
    public_send("#{moving_average_limit_prefix}_timing_condition") == 'after' &&
      active_moving_average_limit_condition_met_at.present?
  end

  def moving_average_condition_satisfied?(price, ma_value)
    case public_send("#{moving_average_limit_prefix}_value_condition")
    when 'below'
      price < ma_value
    when 'above'
      price > ma_value
    else
      false
    end
  end

  def get_moving_average_value(ticker)
    ma_type = public_send("#{moving_average_limit_prefix}_in_ma_type")
    timeframe = MOVING_AVERAGE_LIMIT_TIMEFRAMES[public_send("#{moving_average_limit_prefix}_in_timeframe")]
    period = public_send("#{moving_average_limit_prefix}_in_period")
    case ma_type
    when 'sma'
      ticker.get_sma_value(timeframe:, period:)
    when 'ema'
      ticker.get_ema_value(timeframe:, period:)
    else
      raise "Invalid moving average type: #{ma_type}"
    end
  end

  def initialize_moving_average_limitable_settings
    self.moving_average_limited ||= false
    self.moving_average_limit_timing_condition ||= 'while'
    self.moving_average_limit_value_condition ||= 'below'
    self.moving_average_limit_in_ticker_id ||= tickers.min_by { |t| t[:base] }&.id
    self.moving_average_limit_in_ma_type ||= 'sma'
    self.moving_average_limit_in_timeframe ||= 'one_day'
    self.moving_average_limit_in_period ||= 9
    # Sell-side defaults are read-time fallbacks (see readers above), never written on load.
  end

  def set_moving_average_limit_enabled_at
    if moving_average_limited_was != moving_average_limited
      self.moving_average_limit_enabled_at = moving_average_limited? ? Time.current : nil
    end
    return if sell_moving_average_limited_was == sell_moving_average_limited

    self.sell_moving_average_limit_enabled_at = sell_moving_average_limited? ? Time.current : nil
  end

  def set_moving_average_limit_condition_met_at
    self.moving_average_limit_condition_met_at = nil if moving_average_limited_was != moving_average_limited
    self.sell_moving_average_limit_condition_met_at = nil if sell_moving_average_limited_was != sell_moving_average_limited
  end

  def set_moving_average_limit_in_ticker_id
    if moving_average_limit_in_ticker_id_was.present? && exchange_id_was.present? && exchange_id_was != exchange_id
      ticker_was = Ticker.find_by(id: moving_average_limit_in_ticker_id_was)
      self.moving_average_limit_in_ticker_id = tickers.find_by(
        base_asset_id: ticker_was.base_asset_id,
        quote_asset_id: ticker_was.quote_asset_id
      )&.id
      sell_ticker_was = Ticker.find_by(id: sell_moving_average_limit_in_ticker_id_was) if sell_moving_average_limit_in_ticker_id_was.present?
      self.sell_moving_average_limit_in_ticker_id = if sell_ticker_was
                                                      tickers.find_by(base_asset_id: sell_ticker_was.base_asset_id,
                                                                      quote_asset_id: sell_ticker_was.quote_asset_id)&.id
                                                    else
                                                      tickers.min_by { |t| t[:base] }&.id
                                                    end
    else
      default_ticker_id = tickers.min_by { |t| t[:base] }&.id
      self.moving_average_limit_in_ticker_id = default_ticker_id
      self.sell_moving_average_limit_in_ticker_id = default_ticker_id
    end
  end

  def cancel_scheduled_moving_average_limit_check_jobs
    cancel_solid_queue_jobs(
      job_class: 'Bot::MovingAverageLimitCheckJob',
      record: self
    )
  end
end
