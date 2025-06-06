module Bot::PriceLimitable
  extend ActiveSupport::Concern

  PRICE_LIMIT_TIMING_CONDITIONS = %w[while after].freeze
  PRICE_LIMIT_VALUE_CONDITIONS = %w[above below].freeze

  included do # rubocop:disable Metrics/BlockLength
    store_accessor :settings,
                   :price_limited,
                   :price_limit,
                   :price_limit_timing_condition,
                   :price_limit_value_condition,
                   :price_limit_in_ticker_id
    store_accessor :transient_data,
                   :price_limit_enabled_at,
                   :price_limit_condition_met_at

    after_initialize :initialize_price_limitable_settings

    before_save :set_price_limit_enabled_at, if: :will_save_change_to_settings?
    before_save :set_price_limit_condition_met_at, if: :will_save_change_to_settings?
    before_save :set_price_limit_in_ticker_id, if: :will_save_change_to_exchange_id?
    before_save :reset_price_limit_info_cache, if: :will_save_change_to_settings?

    validates :price_limited, inclusion: { in: [true, false] }
    validates :price_limit, numericality: { greater_than_or_equal_to: 0 }, if: :price_limited?
    validates :price_limit_timing_condition, inclusion: { in: PRICE_LIMIT_TIMING_CONDITIONS }
    validates :price_limit_value_condition, inclusion: { in: PRICE_LIMIT_VALUE_CONDITIONS }
    validate :validate_price_limitable_included_in_subscription_plan, on: :start

    decorators = Module.new do
      def parsed_settings(settings_hash)
        super(settings_hash).merge(
          price_limited: settings_hash[:price_limited].presence&.in?(%w[1 true]),
          price_limit: settings_hash[:price_limit].presence&.to_f,
          price_limit_timing_condition: settings_hash[:price_limit_timing_condition].presence,
          price_limit_value_condition: settings_hash[:price_limit_value_condition].presence,
          price_limit_in_ticker_id: settings_hash[:price_limit_in_ticker_id].presence&.to_i
        ).compact
      end

      def execute_action
        return super unless price_limited?

        result = get_price_limit_condition_met?
        if result.success? && result.data
          super
        else
          update!(status: :waiting)
          next_check_at = Time.now.utc.end_of_minute
          Bot::PriceLimitCheckJob.set(wait_until: next_check_at).perform_later(self)
          Result::Success.new({ break_reschedule: true })
        end
      end

      def stop(stop_message_key: nil)
        is_stopped = super(stop_message_key: stop_message_key)
        return is_stopped unless price_limited?

        cancel_scheduled_price_limit_check_jobs
        is_stopped
      end

      def pending_quote_amount
        return super unless price_limited?

        started_at_bak = started_at
        self.started_at = if started_at.nil? || price_limit_condition_met_at.nil?
                            nil
                          else
                            [started_at, price_limit_condition_met_at].max
                          end
        value = super
        self.started_at = started_at_bak
        value
      end
    end

    prepend decorators
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

  def get_price_limit_condition_met?
    return Result::Success.new(false) unless price_limited?
    return Result::Success.new(true) if timing_condition_satisfied?

    ticker = tickers.find_by(id: price_limit_in_ticker_id)
    return Result::Success.new(false) unless ticker.present?

    result = ticker.get_last_price
    return result if result.failure?

    if value_condition_satisfied?(result.data)
      if price_limit_condition_met_at.nil?
        update!(price_limit_condition_met_at: Time.current)
        broadcast_price_limit_info_update
      end
      Result::Success.new(true)
    else
      if price_limit_condition_met_at.present?
        set_missed_quote_amount
        update!(price_limit_condition_met_at: nil)
        broadcast_price_limit_info_update
      end
      Result::Success.new(false)
    end
  end

  def broadcast_price_limit_info_update
    ticker = tickers.find_by(id: price_limit_in_ticker_id)
    return if ticker.nil?

    price_result = ticker.get_last_price
    return if price_result.failure?
    return unless price_result.data.present?

    condition_met_result = get_price_limit_condition_met?
    return if condition_met_result.failure?

    info = Rails.cache.fetch(price_limit_info_cache_key, expires_in: 20.seconds) do
      {
        base: ticker.base_asset.symbol,
        quote: ticker.quote_asset.symbol,
        price: price_result.data.round(decimals[:quote]),
        condition_met: condition_met_result.data
      }
    end

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: new_record? ? 'new-settings-price-limit-info' : 'settings-price-limit-info',
      partial: 'bots/settings/price_limit_info',
      locals: { bot: self, info: info }
    )
  end

  def price_limit_info_from_cache
    Rails.cache.read(price_limit_info_cache_key)
  end

  private

  def validate_price_limitable_included_in_subscription_plan
    return unless price_limited?
    return if user.subscription.pro? || user.subscription.legendary?

    errors.add(:user, :upgrade)
  end

  def price_limit_info_cache_key
    "bot_#{id}_price_limit_info"
  end

  def reset_price_limit_info_cache
    return if price_limit_was == price_limit &&
              price_limit_value_condition_was == price_limit_value_condition &&
              price_limit_in_ticker_id_was == price_limit_in_ticker_id

    Rails.cache.delete(price_limit_info_cache_key)
  end

  def timing_condition_satisfied?
    price_limit_timing_condition == 'after' && price_limit_condition_met_at.present?
  end

  def value_condition_satisfied?(current_price)
    case price_limit_value_condition
    when 'below'
      current_price < price_limit
    when 'above'
      current_price > price_limit
    else
      false
    end
  end

  def initialize_price_limitable_settings # rubocop:disable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity
    self.price_limited ||= false
    self.price_limit ||= 1_000_000 # 1 million meme
    self.price_limit_timing_condition ||= 'while'
    self.price_limit_value_condition ||= 'below'
    self.price_limit_in_ticker_id ||= tickers&.sort_by { |t| t[:base] }&.first&.id
  end

  def set_price_limit_enabled_at
    return if price_limited_was == price_limited

    self.price_limit_enabled_at = price_limited? ? Time.current : nil
  end

  def set_price_limit_condition_met_at
    return if price_limited_was == price_limited

    self.price_limit_condition_met_at = nil
  end

  def set_price_limit_in_ticker_id # rubocop:disable Metrics/CyclomaticComplexity,Metrics/PerceivedComplexity
    if price_limit_in_ticker_id_was.present? && exchange_id_was.present? && exchange_id_was != exchange_id
      ticker_was = ExchangeTicker.find_by(id: price_limit_in_ticker_id_was)
      self.price_limit_in_ticker_id = tickers&.find_by(
        base_asset_id: ticker_was.base_asset_id,
        quote_asset_id: ticker_was.quote_asset_id
      )&.id
    else
      self.price_limit_in_ticker_id = tickers&.sort_by { |t| t[:base] }&.first&.id
    end
  end

  def cancel_scheduled_price_limit_check_jobs
    sidekiq_places = [
      Sidekiq::ScheduledSet.new,
      Sidekiq::Queue.new(exchange.name_id),
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
