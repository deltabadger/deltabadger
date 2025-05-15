module Bots::Barbell::QuoteAmountLimitable
  extend ActiveSupport::Concern

  included do
    store_accessor :settings, :quote_amount_limited, :quote_amount_limit
    store_accessor :transient_data, :quote_amount_limit_enabled_at

    before_save :set_quote_amount_limit_enabled_at, if: :will_save_change_to_settings?

    validates :quote_amount_limited, inclusion: { in: [true, false] }, if: -> { quote_amount_limited.present? }
    validates :quote_amount_limit, presence: true, if: -> { quote_amount_limited? }
    validates :quote_amount_limit, numericality: { greater_than: 0 }, if: -> { quote_amount_limited? }
    validate :validate_quote_amount_limit_not_reached, if: :quote_amount_limited?, on: :start
  end

  def quote_amount_limit_enabled_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def set_quote_amount_limit_enabled_at
    return unless settings_was['quote_amount_limited'] != quote_amount_limited

    self.quote_amount_limit_enabled_at = quote_amount_limited? ? Time.current : nil
  end

  def quote_amount_limited?
    quote_amount_limited.present? && quote_amount_limited
  end

  def quote_amount_available_before_limit_reached
    return Float::INFINITY unless quote_amount_limit.present?

    quote_amount_spent = transactions.success.where('created_at > ?', quote_amount_limit_enabled_at).sum(:quote_amount)
    [quote_amount_limit - quote_amount_spent, 0].max
  end

  def quote_amount_limit_reached?
    return false unless quote_amount.present? && quote_amount_limit.present?

    quote_amount_limited? && quote_amount_available_before_limit_reached < [
      [quote_amount, quote_amount_limit].min * 0.01, # give a 1% buffer to avoid rounding & fees errors
      1.0 / (10**decimals[:quote])                   # minimum amount to be shown to the user
    ].max
  end

  def validate_quote_amount_limit_not_reached
    errors.add(:settings, :quote_amount_limit_reached) if quote_amount_limit_reached?
  end
end
