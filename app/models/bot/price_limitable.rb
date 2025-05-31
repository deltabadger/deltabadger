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
                   :price_limit_in_asset_id,
                   :price_limit_vs_currency
    store_accessor :transient_data,
                   :price_limit_enabled_at,
                   :price_limit_condition_met_at

    after_initialize :initialize_price_limitable_settings

    before_save :set_price_limit_enabled_at, if: :will_save_change_to_settings?

    validates :price_limited, inclusion: { in: [true, false] }
    validates :price_limit, numericality: { greater_than_or_equal_to: 0 }, if: :price_limited?
    validates :price_limit_timing_condition, inclusion: { in: PRICE_LIMIT_TIMING_CONDITIONS }
    validates :price_limit_value_condition, inclusion: { in: PRICE_LIMIT_VALUE_CONDITIONS }
    validates :price_limit_in_asset_id, inclusion: { in: ->(b) { b.assets.pluck(:id) - [b.quote_asset_id] } }
    validates :price_limit_vs_currency, inclusion: { in: Asset::VS_CURRENCIES }

    decorators = Module.new do
      def parsed_settings(settings_hash)
        super(settings_hash).merge(
          price_limited: settings_hash[:price_limited].presence&.in?(%w[1 true]),
          price_limit: settings_hash[:price_limit].presence&.to_f,
          price_limit_timing_condition: settings_hash[:price_limit_timing_condition].presence,
          price_limit_value_condition: settings_hash[:price_limit_value_condition].presence,
          price_limit_in_asset_id: settings_hash[:price_limit_in_asset_id].presence&.to_i,
          price_limit_vs_currency: settings_hash[:price_limit_vs_currency].presence
        ).compact
      end

      def execute_action
        return super unless price_limited?

        puts "decorator price_limit_condition_met? #{price_limit_condition_met?}"
        if price_limit_condition_met?
          super
        else
          update!(status: :waiting)
          next_check_at = Time.current + Utilities::Time.seconds_to_next_five_minute_cut
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

  def price_limit_condition_met?
    return false unless price_limited?
    return true if timing_condition_satisfied?

    asset = assets.find_by(id: price_limit_in_asset_id)
    return false unless asset.present?

    result = asset.get_price(currency: price_limit_vs_currency)
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
    case price_limit_value_condition
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
    self.price_limit_value_condition ||= 'below'
    self.price_limit_in_asset_id ||= assets&.min_by(&:symbol)&.id
    self.price_limit_vs_currency ||= Asset::VS_CURRENCIES.first
  end

  def set_price_limit_enabled_at
    return if price_limited_was == price_limited

    self.price_limit_enabled_at = price_limited? ? Time.current : nil
  end

  def set_price_limit_condition_met_at
    return if price_limited_was == price_limited

    self.price_limit_condition_met_at = nil
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
