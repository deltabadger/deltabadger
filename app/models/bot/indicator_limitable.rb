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

  included do # rubocop:disable Metrics/BlockLength
    store_accessor :settings,
                   :indicator_limited,
                   :indicator_limit,
                   :indicator_limit_timing_condition,
                   :indicator_limit_value_condition,
                   :indicator_limit_in_ticker_id,
                   :indicator_limit_in_indicator,
                   :indicator_limit_in_timeframe
    store_accessor :transient_data,
                   :indicator_limit_enabled_at,
                   :indicator_limit_condition_met_at

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
    validate :validate_indicator_limitable_included_in_subscription_plan, on: :start

    decorators = Module.new do
      def parsed_settings(settings_hash)
        super(settings_hash).merge(
          indicator_limited: settings_hash[:indicator_limited].presence&.in?(%w[1 true]),
          indicator_limit: settings_hash[:indicator_limit].presence&.to_f,
          indicator_limit_timing_condition: settings_hash[:indicator_limit_timing_condition].presence,
          indicator_limit_value_condition: settings_hash[:indicator_limit_value_condition].presence,
          indicator_limit_in_ticker_id: settings_hash[:indicator_limit_in_ticker_id].presence&.to_i,
          indicator_limit_in_indicator: settings_hash[:indicator_limit_in_indicator].presence,
          indicator_limit_in_timeframe: settings_hash[:indicator_limit_in_timeframe].presence
        ).compact
      end

      def execute_action
        return super unless indicator_limited?

        result = get_indicator_limit_condition_met?
        if result.success? && result.data
          super
        else
          update!(status: :waiting)
          next_check_at = Time.now.utc + Utilities::Time.seconds_to_next_candle_open(indicator_limit_in_timeframe_duration)
          Bot::IndicatorLimitCheckJob.set(wait_until: next_check_at).perform_later(self)
          Result::Success.new({ break_reschedule: true })
        end
      end

      def stop(stop_message_key: nil)
        is_stopped = super(stop_message_key: stop_message_key)
        return is_stopped unless indicator_limited?

        cancel_scheduled_indicator_limit_check_jobs
        is_stopped
      end

      def pending_quote_amount
        return super unless indicator_limited?

        started_at_bak = started_at
        self.started_at = if started_at.nil? || indicator_limit_condition_met_at.nil?
                            nil
                          else
                            [started_at, indicator_limit_condition_met_at].max
                          end
        value = super
        self.started_at = started_at_bak
        value
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

  def indicator_limit_in_timeframe_duration
    INDICATOR_LIMIT_TIMEFRAMES[indicator_limit_in_timeframe]
  end

  def indicator_limited?
    indicator_limited == true
  end

  def get_indicator_limit_condition_met?
    return Result::Success.new(false) unless indicator_limited?
    return Result::Success.new(true) if timing_condition_satisfied?

    ticker = tickers.find_by(id: indicator_limit_in_ticker_id)
    return Result::Success.new(false) unless ticker.present?

    result = get_indicator_value(ticker)
    return result if result.failure?

    if indicator_condition_satisfied?(result.data)
      if indicator_limit_condition_met_at.nil?
        update!(indicator_limit_condition_met_at: Time.current)
        broadcast_indicator_limit_info_update
      end
      Result::Success.new(true)
    else
      if indicator_limit_condition_met_at.present?
        set_missed_quote_amount
        update!(indicator_limit_condition_met_at: nil)
        broadcast_indicator_limit_info_update
      end
      Result::Success.new(false)
    end
  end

  def broadcast_indicator_limit_info_update
    ticker = tickers.find_by(id: indicator_limit_in_ticker_id)
    return if ticker.nil?

    indicator_value_result = get_indicator_value(ticker)
    return if indicator_value_result.failure?
    return unless indicator_value_result.data.present?

    condition_met_result = get_indicator_limit_condition_met?
    return if condition_met_result.failure?

    info = Rails.cache.fetch(indicator_limit_info_cache_key, expires_in: 20.seconds) do
      {
        base: ticker.base_asset.symbol,
        quote: ticker.quote_asset.symbol,
        value: indicator_value_result.data.round(2),
        condition_met: condition_met_result.data
      }
    end

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: new_record? ? 'new-settings-indicator-limit-info' : 'settings-indicator-limit-info',
      partial: 'bots/settings/indicator_limit_info',
      locals: { bot: self, info: info }
    )
  end

  def indicator_limit_info_from_cache
    Rails.cache.read(indicator_limit_info_cache_key)
  end

  private

  def validate_indicator_limitable_included_in_subscription_plan
    return unless indicator_limited?
    return if user.subscription.pro? || user.subscription.legendary?

    errors.add(:user, :upgrade)
  end

  def indicator_limit_info_cache_key
    "bot_#{id}_indicator_limit_info"
  end

  def reset_indicator_limit_info_cache
    return if indicator_limit_was == indicator_limit &&
              indicator_limit_value_condition_was == indicator_limit_value_condition &&
              indicator_limit_in_ticker_id_was == indicator_limit_in_ticker_id &&
              indicator_limit_in_indicator_was == indicator_limit_in_indicator &&
              indicator_limit_in_timeframe_was == indicator_limit_in_timeframe

    Rails.cache.delete(indicator_limit_info_cache_key)
  end

  def timing_condition_satisfied?
    indicator_limit_timing_condition == 'after' && indicator_limit_condition_met_at.present?
  end

  def indicator_condition_satisfied?(value)
    case indicator_limit_value_condition
    when 'below'
      value < indicator_limit
    when 'above'
      value > indicator_limit
    else
      false
    end
  end

  def get_indicator_value(ticker)
    case indicator_limit_in_indicator
    when 'rsi'
      ticker.get_rsi_value(
        timeframe: INDICATOR_LIMIT_TIMEFRAMES[indicator_limit_in_timeframe],
        period: 14
      )
    else
      raise "Unknown indicator: #{indicator_limit_in_indicator}"
    end
  end

  def initialize_indicator_limitable_settings # rubocop:disable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity
    self.indicator_limited ||= false
    self.indicator_limit ||= 30
    self.indicator_limit_timing_condition ||= 'while'
    self.indicator_limit_value_condition ||= 'below'
    self.indicator_limit_in_ticker_id ||= tickers&.sort_by { |t| t[:base] }&.first&.id
    self.indicator_limit_in_indicator ||= 'rsi'
    self.indicator_limit_in_timeframe ||= 'one_day'
  end

  def set_indicator_limit_enabled_at
    return if indicator_limited_was == indicator_limited

    self.indicator_limit_enabled_at = indicator_limited? ? Time.current : nil
  end

  def set_indicator_limit_condition_met_at
    return if indicator_limited_was == indicator_limited

    self.indicator_limit_condition_met_at = nil
  end

  def set_indicator_limit_in_ticker_id # rubocop:disable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity
    if indicator_limit_in_ticker_id_was.present? && exchange_id_was.present? && exchange_id_was != exchange_id
      ticker_was = ExchangeTicker.find_by(id: indicator_limit_in_ticker_id_was)
      self.indicator_limit_in_ticker_id = tickers&.find_by(
        base_asset_id: ticker_was.base_asset_id,
        quote_asset_id: ticker_was.quote_asset_id
      )&.id
    else
      self.indicator_limit_in_ticker_id = tickers&.sort_by { |t| t[:base] }&.first&.id
    end
  end

  def cancel_scheduled_indicator_limit_check_jobs
    sidekiq_places = [
      Sidekiq::ScheduledSet.new,
      Sidekiq::Queue.new(exchange.name_id),
      Sidekiq::RetrySet.new
    ]
    sidekiq_places.each do |place|
      place.each do |job|
        job.delete if job.queue == 'default' &&
                      job.display_class == 'Bot::IndicatorLimitCheckJob' &&
                      job.display_args.first == [{ '_aj_globalid' => to_global_id.to_s }].first
      end
    end
  end
end
