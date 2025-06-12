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

  included do # rubocop:disable Metrics/BlockLength
    store_accessor :settings,
                   :moving_average_limited,
                   :moving_average_limit_timing_condition,
                   :moving_average_limit_value_condition,
                   :moving_average_limit_in_ticker_id,
                   :moving_average_limit_in_ma_type,
                   :moving_average_limit_in_timeframe,
                   :moving_average_limit_in_period
    store_accessor :transient_data,
                   :moving_average_limit_enabled_at,
                   :moving_average_limit_condition_met_at

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
    validate :validate_moving_average_limitable_included_in_subscription_plan, on: :start

    decorators = Module.new do
      def parse_params(params)
        super(params).merge(
          moving_average_limited: params[:moving_average_limited].presence&.in?(%w[1 true]),
          moving_average_limit_timing_condition: params[:moving_average_limit_timing_condition].presence,
          moving_average_limit_value_condition: params[:moving_average_limit_value_condition].presence,
          moving_average_limit_in_ticker_id: params[:moving_average_limit_in_ticker_id].presence&.to_i,
          moving_average_limit_in_ma_type: params[:moving_average_limit_in_ma_type].presence,
          moving_average_limit_in_timeframe: params[:moving_average_limit_in_timeframe].presence,
          moving_average_limit_in_period: params[:moving_average_limit_in_period].presence&.to_i
        ).compact
      end

      def execute_action
        return super unless moving_average_limited?

        result = get_moving_average_limit_condition_met?
        if result.success? && result.data
          super
        else
          update!(status: :waiting)
          next_check_at = Time.now.utc + Utilities::Time.seconds_to_current_candle_close(moving_average_limit_in_timeframe_duration)
          Bot::MovingAverageLimitCheckJob.set(wait_until: next_check_at).perform_later(self)
          Result::Success.new({ break_reschedule: true })
        end
      end

      def stop(stop_message_key: nil)
        is_stopped = super(stop_message_key: stop_message_key)
        return is_stopped unless moving_average_limited?

        cancel_scheduled_moving_average_limit_check_jobs
        is_stopped
      end

      def started_at
        return super unless moving_average_limited?

        if super.nil? || moving_average_limit_condition_met_at.nil?
          nil
        else
          [super, moving_average_limit_condition_met_at].max
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

  def moving_average_limit_in_timeframe_duration
    MOVING_AVERAGE_LIMIT_TIMEFRAMES[moving_average_limit_in_timeframe]
  end

  def moving_average_limited?
    moving_average_limited == true
  end

  def get_moving_average_limit_condition_met?
    return Result::Success.new(false) unless moving_average_limited?
    return Result::Success.new(true) if timing_condition_satisfied?

    ticker = tickers.find_by(id: moving_average_limit_in_ticker_id)
    return Result::Success.new(false) unless ticker.present?

    price_result = ticker.get_last_price
    return price_result if price_result.failure?

    moving_average_value_result = get_moving_average_value(ticker)
    return moving_average_value_result if moving_average_value_result.failure?

    if moving_average_condition_satisfied?(price_result.data, moving_average_value_result.data)
      if moving_average_limit_condition_met_at.nil?
        update!(moving_average_limit_condition_met_at: Time.current)
        broadcast_moving_average_limit_info_update
      end
      Result::Success.new(true)
    else
      if moving_average_limit_condition_met_at.present?
        set_missed_quote_amount
        update!(moving_average_limit_condition_met_at: nil)
        broadcast_moving_average_limit_info_update
      end
      Result::Success.new(false)
    end
  end

  def broadcast_moving_average_limit_info_update
    ticker = tickers.find_by(id: moving_average_limit_in_ticker_id)
    return if ticker.nil?

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
      locals: { bot: self, info: info }
    )
  end

  def moving_average_limit_info_from_cache
    Rails.cache.read(moving_average_limit_info_cache_key)
  end

  private

  def validate_moving_average_limitable_included_in_subscription_plan
    return unless moving_average_limited?
    return if user.subscription.pro? || user.subscription.legendary?

    errors.add(:user, :upgrade)
  end

  def moving_average_limit_info_cache_key
    "bot_#{id}_moving_average_limit_info"
  end

  def reset_moving_average_limit_info_cache
    return if moving_average_limit_value_condition_was == moving_average_limit_value_condition &&
              moving_average_limit_in_ticker_id_was == moving_average_limit_in_ticker_id &&
              moving_average_limit_in_ma_type_was == moving_average_limit_in_ma_type &&
              moving_average_limit_in_timeframe_was == moving_average_limit_in_timeframe &&
              moving_average_limit_in_period_was == moving_average_limit_in_period

    Rails.cache.delete(moving_average_limit_info_cache_key)
  end

  def timing_condition_satisfied?
    moving_average_limit_timing_condition == 'after' && moving_average_limit_condition_met_at.present?
  end

  def moving_average_condition_satisfied?(price, ma_value)
    case moving_average_limit_value_condition
    when 'below'
      price < ma_value
    when 'above'
      price > ma_value
    else
      false
    end
  end

  def get_moving_average_value(ticker)
    case moving_average_limit_in_ma_type
    when 'sma'
      ticker.get_sma_value(timeframe: MOVING_AVERAGE_LIMIT_TIMEFRAMES[moving_average_limit_in_timeframe],
                           period: moving_average_limit_in_period)
    when 'ema'
      ticker.get_ema_value(timeframe: MOVING_AVERAGE_LIMIT_TIMEFRAMES[moving_average_limit_in_timeframe],
                           period: moving_average_limit_in_period)
    else
      raise "Invalid moving average type: #{moving_average_limit_in_ma_type}"
    end
  end

  def initialize_moving_average_limitable_settings # rubocop:disable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity
    self.moving_average_limited ||= false
    self.moving_average_limit_timing_condition ||= 'while'
    self.moving_average_limit_value_condition ||= 'below'
    self.moving_average_limit_in_ticker_id ||= tickers&.sort_by { |t| t[:base] }&.first&.id
    self.moving_average_limit_in_ma_type ||= 'sma'
    self.moving_average_limit_in_timeframe ||= 'one_day'
    self.moving_average_limit_in_period ||= 9
  end

  def set_moving_average_limit_enabled_at
    return if moving_average_limited_was == moving_average_limited

    self.moving_average_limit_enabled_at = moving_average_limited? ? Time.current : nil
  end

  def set_moving_average_limit_condition_met_at
    return if moving_average_limited_was == moving_average_limited

    self.moving_average_limit_condition_met_at = nil
  end

  def set_moving_average_limit_in_ticker_id # rubocop:disable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity
    if moving_average_limit_in_ticker_id_was.present? && exchange_id_was.present? && exchange_id_was != exchange_id
      ticker_was = ExchangeTicker.find_by(id: moving_average_limit_in_ticker_id_was)
      self.moving_average_limit_in_ticker_id = tickers&.find_by(
        base_asset_id: ticker_was.base_asset_id,
        quote_asset_id: ticker_was.quote_asset_id
      )&.id
    else
      self.moving_average_limit_in_ticker_id = tickers&.sort_by { |t| t[:base] }&.first&.id
    end
  end

  def cancel_scheduled_moving_average_limit_check_jobs
    sidekiq_places = [
      Sidekiq::ScheduledSet.new,
      Sidekiq::Queue.new(exchange.name_id),
      Sidekiq::RetrySet.new
    ]
    sidekiq_places.each do |place|
      place.each do |job|
        job.delete if job.queue == 'default' &&
                      job.display_class == 'Bot::MovingAverageLimitCheckJob' &&
                      job.display_args.first == [{ '_aj_globalid' => to_global_id.to_s }].first
      end
    end
  end
end
