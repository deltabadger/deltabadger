module Bot::SmartIntervalable
  extend ActiveSupport::Concern

  included do # rubocop:disable Metrics/BlockLength
    store_accessor :settings,
                   :smart_intervaled,
                   :smart_interval_quote_amount

    after_initialize :initialize_smart_intervalable_settings
    before_save :readjust_smart_interval_quote_amount, if: :will_save_change_to_settings?

    validates :smart_intervaled, inclusion: { in: [true, false] }
    validates :smart_interval_quote_amount,
              numericality: { greater_than_or_equal_to: :minimum_smart_interval_quote_amount },
              if: :smart_intervaled
    validate :validate_smart_intervalable_included_in_subscription_plan, on: :start

    decorators = Module.new do
      def parse_params(params)
        super(params).merge(
          smart_intervaled: params[:smart_intervaled].presence&.in?(%w[1 true]),
          smart_interval_quote_amount: params[:smart_interval_quote_amount].presence&.to_f
        ).compact
      end

      def effective_quote_amount
        return super unless smart_intervaled? &&
                            smart_interval_quote_amount.present?

        smart_interval_quote_amount
      end

      def effective_interval_duration
        return super unless smart_intervaled? &&
                            smart_interval_quote_amount.present? &&
                            quote_amount.present?

        # effective_interval_duration is an ActiveSupport::Duration. However, for some durations, after this
        # division, addition in other methods (e.g. Time.current + effective_interval_duration) stops working.
        # Re-converting it to seconds makes the addition work. Do NOT remove the .seconds !
        (super / (quote_amount.to_f / smart_interval_quote_amount)).seconds
      end
    end

    prepend decorators
  end

  def smart_intervaled?
    smart_intervaled == true
  end

  private

  def validate_smart_intervalable_included_in_subscription_plan
    return unless smart_intervaled?
    return if user.subscription.paid?

    errors.add(:user, :upgrade)
  end

  def initialize_smart_intervalable_settings
    self.smart_intervaled ||= false
    self.smart_interval_quote_amount ||= if quote_amount.present? && tickers.present?
                                           least_precise_quote_decimals = tickers.pluck(:quote_decimals).compact.min
                                           [
                                             quote_amount / 10,
                                             minimum_smart_interval_quote_amount * 10
                                           ].max.round(least_precise_quote_decimals)
                                         end
  end

  def minimum_smart_interval_quote_amount
    # the minimum amount would set one order every 1 minute
    maximum_frequency = 300 # seconds
    minimum_for_frequency = if quote_amount.present?
                              quote_amount / interval_duration.to_f * maximum_frequency
                            else
                              0
                            end

    least_precise_quote_decimals = tickers.pluck(:quote_decimals).compact.min
    minimum_for_precision = 1.0 / (10**least_precise_quote_decimals)

    [
      Utilities::Number.round_up(minimum_for_frequency, precision: least_precise_quote_decimals),
      minimum_for_precision
    ].max
  end

  def readjust_smart_interval_quote_amount
    return unless smart_intervaled?
    return unless quote_amount_was.present? && quote_amount.present? && quote_amount_was != quote_amount

    previous_ratio = quote_amount_was.to_f / smart_interval_quote_amount_was
    least_precise_quote_decimals = tickers.pluck(:quote_decimals).compact.min
    self.smart_interval_quote_amount = [
      quote_amount.to_f / previous_ratio,
      minimum_smart_interval_quote_amount
    ].max.round(least_precise_quote_decimals)
  end
end
