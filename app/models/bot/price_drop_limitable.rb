module Bot::PriceDropLimitable
  extend ActiveSupport::Concern

  PRICE_DROP_LIMIT_TIME_WINDOW_CONDITIONS = {
    'ath' => Float::INFINITY,
    'twenty_four_hours' => 24.hours
  }.freeze

  included do # rubocop:disable Metrics/BlockLength
    store_accessor :settings,
                   :price_drop_limited,
                   :price_drop_limit,
                   :price_drop_limit_time_window_condition,
                   :price_drop_limit_in_ticker_id
    store_accessor :transient_data,
                   :price_drop_limit_enabled_at,
                   :price_drop_limit_condition_met_at

    after_initialize :initialize_price_drop_limitable_settings

    before_save :set_price_drop_limit_enabled_at, if: :will_save_change_to_settings?
    before_save :set_price_drop_limit_condition_met_at, if: :will_save_change_to_settings?
    before_save :set_price_drop_limit_in_ticker_id, if: :will_save_change_to_exchange_id?
    before_save :reset_price_drop_limit_info_cache, if: :will_save_change_to_settings?

    validates :price_drop_limited, inclusion: { in: [true, false] }
    validates :price_drop_limit,
              numericality: {
                greater_than_or_equal_to: 0,
                less_than_or_equal_to: 1
              },
              if: :price_drop_limited?
    validates :price_drop_limit_time_window_condition, inclusion: { in: PRICE_DROP_LIMIT_TIME_WINDOW_CONDITIONS.keys }
    validate :validate_price_drop_limitable_included_in_subscription_plan, on: :start

    decorators = Module.new do
      def parse_params(params)
        parsed_price_drop_limit = params[:price_drop_limit].presence&.to_f
        parsed_price_drop_limit = parsed_price_drop_limit.present? ? (parsed_price_drop_limit / 100).round(4) : nil
        super(params).merge(
          price_drop_limited: params[:price_drop_limited].presence&.in?(%w[1 true]),
          price_drop_limit: parsed_price_drop_limit,
          price_drop_limit_time_window_condition: params[:price_drop_limit_time_window_condition].presence,
          price_drop_limit_in_ticker_id: params[:price_drop_limit_in_ticker_id].presence&.to_i
        ).compact
      end

      def execute_action
        return super unless price_drop_limited?

        result = get_price_drop_limit_condition_met?
        if result.success? && result.data
          super
        else
          update!(status: :waiting)
          next_check_at = Time.now.utc.end_of_minute
          Bot::PriceDropLimitCheckJob.set(wait_until: next_check_at).perform_later(self)
          Result::Success.new({ break_reschedule: true })
        end
      end

      def stop(stop_message_key: nil)
        is_stopped = super(stop_message_key: stop_message_key)
        return is_stopped unless price_drop_limited?

        cancel_scheduled_price_drop_limit_check_jobs
        is_stopped
      end

      def pending_quote_amount
        return super unless price_drop_limited?

        started_at_bak = started_at
        self.started_at = if started_at.nil? || price_drop_limit_condition_met_at.nil?
                            nil
                          else
                            [started_at, price_drop_limit_condition_met_at].max
                          end
        value = super
        self.started_at = started_at_bak
        value
      end
    end

    prepend decorators
  end

  def price_drop_limit_enabled_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def price_drop_limit_condition_met_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def price_drop_limited?
    price_drop_limited == true
  end

  def price_drop_limit_time_window_duration
    PRICE_DROP_LIMIT_TIME_WINDOW_CONDITIONS[price_drop_limit_time_window_condition]
  end

  def get_price_drop_limit_condition_met? # rubocop:disable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity
    return Result::Success.new(false) unless price_drop_limited?
    return Result::Success.new(true) if timing_condition_satisfied?

    ticker = tickers.find_by(id: price_drop_limit_in_ticker_id)
    return Result::Success.new(false) unless ticker.present?

    price_result = ticker.get_last_price
    return price_result if price_result.failure?

    high_result = ticker.get_high_of_last(duration: price_drop_limit_time_window_duration)
    return high_result if high_result.failure?

    if price_drop_limit_time_window_condition_satisfied?(price_result.data, high_result.data)
      if price_drop_limit_condition_met_at.nil?
        update!(price_drop_limit_condition_met_at: Time.current)
        broadcast_price_drop_limit_info_update
      end
      Result::Success.new(true)
    else
      if price_drop_limit_condition_met_at.present?
        set_missed_quote_amount
        update!(price_drop_limit_condition_met_at: nil)
        broadcast_price_drop_limit_info_update
      end
      Result::Success.new(false)
    end
  end

  def broadcast_price_drop_limit_info_update
    ticker = tickers.find_by(id: price_drop_limit_in_ticker_id)
    return if ticker.nil?

    high_result = ticker.get_high_of_last(duration: price_drop_limit_time_window_duration)
    return if high_result.failure?
    return unless high_result.data.present?

    condition_met_result = get_price_drop_limit_condition_met?
    return if condition_met_result.failure?

    info = Rails.cache.fetch(price_drop_limit_info_cache_key, expires_in: 20.seconds) do
      {
        base: ticker.base_asset.symbol,
        quote: ticker.quote_asset.symbol,
        high: high_result.data,
        condition_met: condition_met_result.data
      }
    end

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: new_record? ? 'new-settings-price-drop-limit-info' : 'settings-price-drop-limit-info',
      partial: 'bots/settings/price_drop_limit_info',
      locals: { bot: self, info: info }
    )
  end

  def price_drop_limit_info_from_cache
    Rails.cache.read(price_drop_limit_info_cache_key)
  end

  private

  def validate_price_drop_limitable_included_in_subscription_plan
    return unless price_drop_limited?
    return if user.subscription.pro? || user.subscription.legendary?

    errors.add(:user, :upgrade)
  end

  def price_drop_limit_info_cache_key
    "bot_#{id}_price_drop_limit_info"
  end

  def reset_price_drop_limit_info_cache
    return if price_drop_limit_was == price_drop_limit &&
              price_drop_limit_time_window_condition_was == price_drop_limit_time_window_condition &&
              price_drop_limit_in_ticker_id_was == price_drop_limit_in_ticker_id

    Rails.cache.delete(price_drop_limit_info_cache_key)
  end

  def timing_condition_satisfied?
    price_drop_limit_condition_met_at.present?
  end

  def price_drop_limit_time_window_condition_satisfied?(current_price, high_price)
    current_price < (1 - price_drop_limit) * high_price
  end

  def initialize_price_drop_limitable_settings # rubocop:disable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity
    self.price_drop_limited ||= false
    self.price_drop_limit ||= 0.2
    self.price_drop_limit_time_window_condition ||= 'ath'
    self.price_drop_limit_in_ticker_id ||= tickers&.sort_by { |t| t[:base] }&.first&.id
  end

  def set_price_drop_limit_enabled_at
    return if price_drop_limited_was == price_drop_limited

    self.price_drop_limit_enabled_at = price_drop_limited? ? Time.current : nil
  end

  def set_price_drop_limit_condition_met_at
    return if price_drop_limited_was == price_drop_limited

    self.price_drop_limit_condition_met_at = nil
  end

  def set_price_drop_limit_in_ticker_id # rubocop:disable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity
    if price_drop_limit_in_ticker_id_was.present? && exchange_id_was.present? && exchange_id_was != exchange_id
      ticker_was = ExchangeTicker.find_by(id: price_drop_limit_in_ticker_id_was)
      self.price_drop_limit_in_ticker_id = tickers&.find_by(
        base_asset_id: ticker_was.base_asset_id,
        quote_asset_id: ticker_was.quote_asset_id
      )&.id
    else
      self.price_drop_limit_in_ticker_id = tickers&.sort_by { |t| t[:base] }&.first&.id
    end
  end

  def cancel_scheduled_price_drop_limit_check_jobs
    sidekiq_places = [
      Sidekiq::ScheduledSet.new,
      Sidekiq::Queue.new(exchange.name_id),
      Sidekiq::RetrySet.new
    ]
    sidekiq_places.each do |place|
      place.each do |job|
        job.delete if job.queue == 'default' &&
                      job.display_class == 'Bot::PriceDropLimitCheckJob' &&
                      job.display_args.first == [{ '_aj_globalid' => to_global_id.to_s }].first
      end
    end
  end
end
