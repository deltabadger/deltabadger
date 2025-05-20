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
    # validates :quote_amount_limit, numericality: { greater_than_or_equal_to: 0 }
    validates :quote_amount_limit, numericality: { greater_than_or_equal_to: lambda { |b|
                                                                               b.minimum_quote_amount_limit
                                                                             } }, if: :quote_amount_limited?
    validate :validate_quote_amount_limit_not_reached, if: :quote_amount_limited?, on: :start

    execute_action_decorator = Module.new do
      def execute_action
        result = super
        broadcast_quote_amount_limit_update
        return result unless quote_amount_limited? && quote_amount_limit_reached?

        Bot::StopJob.perform_later(self, stop_message_key: 'bot.settings.extra_amount_limit.amount_spent')
        notify_stopped_by_amount_limit
        Result::Success.new({ break_reschedule: true })
      end
    end

    prepend execute_action_decorator
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
    return Float::INFINITY if quote_amount_limit.blank?

    quote_amount_spent = transactions.success.where('created_at > ?', quote_amount_limit_enabled_at).sum(:quote_amount)
    [quote_amount_limit - quote_amount_spent, 0].max
  end

  def quote_amount_limit_reached?
    return false unless quote_amount_limited?

    quote_amount_limited? && quote_amount_available_before_limit_reached < minimum_quote_amount_limit
  end

  def validate_quote_amount_limit_not_reached
    errors.add(:settings, :quote_amount_limit_reached) if quote_amount_limit_reached?
  end

  def stop_and_notify_if_quote_amount_limit_reached
    return unless quote_amount_limit_reached?

    Bot::StopJob.perform_later(self, stop_message_key: 'bot.settings.extra_amount_limit.amount_spent')
    notify_stopped_by_amount_limit
  end

  def broadcast_quote_amount_limit_update
    return unless quote_amount_limited?

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: 'settings-amount-limit-info',
      partial: 'bots/settings/amount_limit_info',
      locals: { bot: self }
    )
  end

  def minimum_quote_amount_limit
    least_precise_quote_decimals = [ticker0&.quote_decimals, ticker1&.quote_decimals].compact.min
    @minimum_quote_amount_limit ||= 1.0 / (10**least_precise_quote_decimals)
  end

  private

  def initialize_quote_amount_limitable_settings
    self.quote_amount_limited ||= false
    self.quote_amount_limit ||= nil
  end

  def set_quote_amount_limit_enabled_at
    return unless settings_was['quote_amount_limited'] != quote_amount_limited

    self.quote_amount_limit_enabled_at = quote_amount_limited? ? Time.current : nil
  end
end
