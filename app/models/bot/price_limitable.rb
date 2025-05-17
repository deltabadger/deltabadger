module Bot::PriceLimitable
  extend ActiveSupport::Concern

  TIMING_CONDITIONS = %w[while after].freeze
  PRICE_CONDITIONS = %w[above below].freeze

  included do # rubocop:disable Metrics/BlockLength
    store_accessor :settings,
                   :price_limited,
                   :price_limit,
                   :price_limit_timing_condition,
                   :price_limit_price_condition,
                   :price_limit_in_ticker_id
    store_accessor :transient_data,
                   :price_limit_enabled_at,
                   :price_limit_condition_met_at

    after_initialize :initialize_price_limitable_settings

    before_save :set_price_limit_enabled_at, if: :will_save_change_to_settings?

    validates :price_limited, inclusion: { in: [true, false] }
    validates :price_limit, numericality: { greater_than: 0 }, if: :price_limited?
    validates :price_limit_timing_condition, inclusion: { in: TIMING_CONDITIONS }
    validates :price_limit_price_condition, inclusion: { in: PRICE_CONDITIONS }
    validates :price_limit_in_ticker_id, inclusion: { in: ->(b) { b.tickers.pluck(:id).compact } }

    execute_action_decorator = Module.new do
      def execute_action
        return super unless price_limited?

        if price_limit_condition_met?
          super
        else
          update!(status: :waiting)
          next_check_at = Time.current + Utilities::Time.seconds_to_end_of_minute
          Bot::PriceLimitCheckJob.set(wait_until: next_check_at).perform_later(self)
          Result::Success.new({ break_reschedule: true })
        end
      end
    end

    prepend execute_action_decorator

    stop_decorator = Module.new do
      def stop(stop_message_key: nil)
        is_stopped = super(stop_message_key: stop_message_key)
        return is_stopped unless price_limited?

        cancel_scheduled_price_limit_check_jobs
        is_stopped
      end
    end

    prepend stop_decorator

    pending_quote_amount_decorator = Module.new do
      def pending_quote_amount(before_settings_change: false)
        return super unless price_limited?

        started_at_was = started_at
        self.started_at = price_limit_condition_met_at if price_limited?
        value = super(before_settings_change: before_settings_change)
        self.started_at = started_at_was
        value
      end
    end

    prepend pending_quote_amount_decorator
  end

  def price_limit_enabled_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def price_limit_condition_met_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def price_limited?
    price_limited == true
  end

  def price_limit_condition_met?
    return false unless price_limited?
    return true if timing_condition_satisfied?

    ticker = tickers.find_by(id: price_limit_in_ticker_id)
    return false unless ticker.present?

    result = ticker.get_price
    return false unless result.success?

    if price_condition_satisfied?(result.data)
      self.price_limit_condition_met_at ||= Time.current
      true
    else
      self.price_limit_condition_met_at = nil
      false
    end
  end

  private

  def timing_condition_satisfied?
    price_limit_timing_condition == 'after' && price_limit_condition_met_at.present?
  end

  def price_condition_satisfied?(current_price)
    case price_limit_price_condition
    when 'below'
      current_price < price_limit
    when 'above'
      current_price > price_limit
    else
      false
    end
  end

  def initialize_price_limitable_settings
    self.price_limited ||= false
    self.price_limit ||= nil
    self.price_limit_timing_condition ||= 'while'
    self.price_limit_price_condition ||= 'below'
    self.price_limit_in_ticker_id ||= tickers&.first&.id
  end

  def set_price_limit_enabled_at
    return unless settings_was['price_limited'] != price_limited

    self.price_limit_enabled_at = price_limited? ? Time.current : nil
  end

  def set_price_limit_condition_met_at
    return unless settings_was['price_limited'] != price_limited

    self.price_limit_condition_met_at = nil
  end

  def cancel_scheduled_price_limit_check_jobs
    sidekiq_places = [
      Sidekiq::ScheduledSet.new,
      Sidekiq::Queue.new(exchange.name.downcase),
      Sidekiq::RetrySet.new
    ]
    sidekiq_places.each do |place|
      place.each do |job|
        job.delete if job.queue == 'default' &&
                      job.display_class == 'Bot::PriceLimitCheckJob' &&
                      job.display_args.first == [{ '_aj_globalid' => to_global_id.to_s }].first
      end
    end
  end
end
