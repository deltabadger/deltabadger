module Bot::PriceLimitable
  extend ActiveSupport::Concern

  TIMING_CONDITIONS = %w[while after].freeze
  PRICE_CONDITIONS = %w[below above].freeze

  included do
    store_accessor :settings,
                   :price_limited,
                   :price_limit,
                   :price_limit_timing_condition,
                   :price_limit_price_condition,
                   :price_limit_in_ticker_id
    store_accessor :transient_data, :price_limit_enabled_at

    after_initialize :initialize_price_limitable_settings

    before_save :set_price_limit_enabled_at, if: :will_save_change_to_settings?

    validates :price_limited, inclusion: { in: [true, false] }
    validates :price_limit, numericality: { greater_than_or_equal_to: 0 }
    validates :price_limit, numericality: { greater_than: 0 }, if: :price_limited?
    validates :price_limit_timing_condition, inclusion: { in: TIMING_CONDITIONS }
    validates :price_limit_price_condition, inclusion: { in: PRICE_CONDITIONS }
    validates :price_limit_in_ticker_id, inclusion: { in: ->(b) { b.tickers.pluck(:id).compact } }
  end

  def price_limit_enabled_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def price_limited?
    price_limited == true
  end

  private

  def initialize_price_limitable_settings
    self.price_limited ||= false
    self.price_limit ||= 0
    self.price_limit_timing_condition ||= TIMING_CONDITIONS.first
    self.price_limit_price_condition ||= PRICE_CONDITIONS.first
    self.price_limit_in_ticker_id ||= tickers&.first&.id
  end

  def set_price_limit_enabled_at
    return unless settings_was['price_limited'] != price_limited

    self.price_limit_enabled_at = price_limited? ? Time.current : nil
  end
end
