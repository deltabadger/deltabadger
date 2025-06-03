module Bot::IndicatorLimitable
  extend ActiveSupport::Concern

  INDICATOR_LIMIT_TIMING_CONDITIONS = %w[while after].freeze
  INDICATOR_LIMIT_VALUE_CONDITIONS = %w[above below].freeze
  INDICATOR_LIMIT_INDICATORS = %w[rsi].freeze

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
    validates :indicator_limit_in_timeframe, inclusion: { in: [1.day] }

    decorators = Module.new do
      def parsed_settings(settings_hash)
        super(settings_hash).merge(
          indicator_limited: settings_hash[:indicator_limited].presence&.in?(%w[1 true]),
          indicator_limit: settings_hash[:indicator_limit].presence&.to_f,
          indicator_limit_timing_condition: settings_hash[:indicator_limit_timing_condition].presence,
          indicator_limit_value_condition: settings_hash[:indicator_limit_value_condition].presence,
          indicator_limit_in_ticker_id: settings_hash[:indicator_limit_in_ticker_id].presence&.to_i,
          indicator_limit_in_indicator: settings_hash[:indicator_limit_in_indicator].presence
        ).compact
      end

      def execute_action
        return super unless indicator_limited?

        result = get_indicator_limit_condition_met?
        if result.success? && result.data
          super
        else
          update!(status: :waiting)
          next_check_at = Time.now.utc + Utilities::Time.seconds_to_next_candle_open(indicator_limit_in_timeframe)
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

  def indicator_limit_in_timeframe
    value = super
    value.present? ? value.seconds : nil
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
      self.indicator_limit_condition_met_at ||= Time.current
      Result::Success.new(true)
    else
      set_missed_quote_amount
      self.indicator_limit_condition_met_at = nil
      Result::Success.new(false)
    end
  end

  private

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
      ticker.get_rsi_value(timeframe: indicator_limit_in_timeframe, period: 14)
    else
      raise "Unknown indicator: #{indicator_limit_in_indicator}"
    end
  end

  def initialize_indicator_limitable_settings # rubocop:disable Metrics/CyclomaticComplexity
    self.indicator_limited ||= false
    self.indicator_limit ||= nil
    self.indicator_limit_timing_condition ||= 'while'
    self.indicator_limit_value_condition ||= 'below'
    self.indicator_limit_in_ticker_id ||= set_indicator_limit_in_ticker_id
    self.indicator_limit_in_indicator ||= 'rsi'
    self.indicator_limit_in_timeframe ||= 1.day
  end

  def set_indicator_limit_enabled_at
    return if indicator_limited_was == indicator_limited

    self.indicator_limit_enabled_at = indicator_limited? ? Time.current : nil
  end

  def set_indicator_limit_condition_met_at
    return if indicator_limited_was == indicator_limited

    self.indicator_limit_condition_met_at = nil
  end

  def set_indicator_limit_in_ticker_id
    self.indicator_limit_in_ticker_id = tickers&.sort_by { |t| t[:base] }&.first&.id
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
