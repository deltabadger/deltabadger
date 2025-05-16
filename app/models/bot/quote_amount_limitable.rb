module Bot::QuoteAmountLimitable
  extend ActiveSupport::Concern

  included do
    store_accessor :settings,
                   :quote_amount_limited,
                   :quote_amount_limit
    store_accessor :transient_data,
                   :quote_amount_limit_enabled_at

    after_initialize :initialize_quote_amount_limitable_settings

    before_save :set_quote_amount_limit_enabled_at, if: :will_save_change_to_settings?

    validates :quote_amount_limited, inclusion: { in: [true, false] }
    validates :quote_amount_limit, numericality: { greater_than_or_equal_to: 0 }
    validates :quote_amount_limit, numericality: { greater_than: 0 }, if: :quote_amount_limited?
    validate :validate_quote_amount_limit_not_reached, if: :quote_amount_limited?, on: :start
  end

  def quote_amount_limit_enabled_at
    value = super
    value.present? ? Time.zone.parse(value) : nil
  end

  def quote_amount_limited?
    quote_amount_limited == true
  end

  def quote_amount_available_before_limit_reached
    return Float::INFINITY unless quote_amount_limited?

    quote_amount_spent = transactions.success.where('created_at > ?', quote_amount_limit_enabled_at).sum(:quote_amount)
    [quote_amount_limit - quote_amount_spent, 0].max
  end

  def quote_amount_limit_reached?
    return false unless quote_amount_limited?
    return true if quote_amount_limit.zero?

    quote_amount_limited? && quote_amount_available_before_limit_reached < [
      [quote_amount || Float::INFINITY, quote_amount_limit].min * 0.01, # 1% buffer to avoid rounding & fees errors
      1.0 / (10**decimals[:quote])                   # minimum amount to be shown to the user
    ].max
  end

  def validate_quote_amount_limit_not_reached
    errors.add(:settings, :quote_amount_limit_reached) if quote_amount_limit_reached?
  end

  def stop_and_notify_if_quote_amount_limit_reached
    return unless quote_amount_limit_reached?

    Bot::StopJob.perform_later(self, stop_message_key: 'bot.settings.extra_amount_limit.amount_spent')
    notify_stopped_by_amount_limit
  end

  private

  def initialize_quote_amount_limitable_settings
    self.quote_amount_limited ||= false
    self.quote_amount_limit ||= 0
  end

  def set_quote_amount_limit_enabled_at
    return unless settings_was['quote_amount_limited'] != quote_amount_limited

    self.quote_amount_limit_enabled_at = quote_amount_limited? ? Time.current : nil
  end
end
