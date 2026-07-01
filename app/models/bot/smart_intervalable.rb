module Bot::SmartIntervalable
  extend ActiveSupport::Concern

  included do
    store_accessor :settings,
                   :smart_intervaled,
                   :smart_interval_quote_amount,
                   :smart_interval_base_amount

    after_initialize :initialize_smart_intervalable_settings
    # The sell base split can't be seeded at load when sell_amount is still blank (e.g. just after a
    # flip). Seed it the moment a sell amount exists — before validation — so entering the sell amount
    # in the main sentence never trips the base-split presence check.
    before_validation :seed_smart_interval_base_amount, if: -> { smart_intervaled? && selling? }

    validates :smart_intervaled, inclusion: { in: [true, false] }
    # Quote split governs buying; base split governs selling. Direction-gate so a selling bot is never
    # blocked by a missing quote amount (and vice versa). The base check additionally waits for a sell
    # amount, so flipping a smart-on bot with no sell sentence yet can't fail validation.
    validates :smart_interval_quote_amount,
              numericality: { greater_than_or_equal_to: :minimum_smart_interval_quote_amount },
              if: -> { smart_intervaled? && !selling? }
    validates :smart_interval_base_amount,
              numericality: { greater_than_or_equal_to: :minimum_smart_interval_base_amount },
              if: -> { smart_intervaled? && selling? && try(:sell_amount).present? }

    decorators = Module.new do
      def parse_params(params)
        super(params).merge(
          smart_intervaled: params[:smart_intervaled].presence&.in?(%w[1 true]),
          smart_interval_quote_amount: params[:smart_interval_quote_amount].presence&.to_f,
          smart_interval_base_amount: params[:smart_interval_base_amount].presence&.to_f
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

  def initialize_smart_intervalable_settings
    self.smart_intervaled ||= false
    self.smart_interval_quote_amount ||= if quote_amount.present? && tickers.present?
                                           [
                                             quote_amount / 10,
                                             minimum_smart_interval_quote_amount * 10
                                           ].max.round(least_precise_quote_decimals)
                                         end
    # The sell base split is NOT seeded here (that would write settings on load and dirty an existing
    # bot). It is seeded by before_validation :seed_smart_interval_base_amount, which only runs while
    # selling+smart with a sell amount — exactly when it's needed and safe to persist.
  end

  # Seed the sell base split from the sell amount (mirror of the buy seed) when it's needed but still
  # blank, so the user never has to open the Smart Intervals rule before setting a sell amount.
  def seed_smart_interval_base_amount
    return if smart_interval_base_amount.present?
    return if try(:sell_amount).blank?

    self.smart_interval_base_amount = [
      sell_amount / 10,
      minimum_smart_interval_base_amount * 10
    ].max.round(least_precise_base_decimals)
  end

  def minimum_smart_interval_base_amount
    return 0 if tickers.empty?

    maximum_frequency = 300 # seconds — at most one sale every 5 minutes
    sell_amount_value = try(:sell_amount).to_f
    interval_secs = (Automation::Schedulable::INTERVALS[try(:sell_interval)] || interval_duration).to_f
    minimum_for_frequency = sell_amount_value.positive? && interval_secs.positive? ? sell_amount_value / interval_secs * maximum_frequency : 0
    minimum_for_precision = 1.0 / (10**least_precise_base_decimals)

    [
      Utilities::Number.round_up(minimum_for_frequency, precision: least_precise_base_decimals),
      minimum_for_precision
    ].max
  end

  def least_precise_base_decimals
    @least_precise_base_decimals ||= tickers.pluck(:base_decimals).compact.min
  end

  def minimum_smart_interval_quote_amount
    return 0 if tickers.empty?

    # the minimum amount would set one order every 5 minutes
    maximum_frequency = 300 # seconds
    minimum_for_frequency = if quote_amount.present?
                              quote_amount / interval_duration.to_f * maximum_frequency
                            else
                              0
                            end

    minimum_for_precision = 1.0 / (10**least_precise_quote_decimals)

    [
      Utilities::Number.round_up(minimum_for_frequency, precision: least_precise_quote_decimals),
      minimum_for_precision,
      minimum_for_exchange
    ].max
  end

  # Override in subclasses to set exchange-specific minimums
  # For Index bots, this returns the highest minimum_quote_size among tickers
  def minimum_for_exchange
    0
  end

  def least_precise_quote_decimals
    @least_precise_quote_decimals ||= tickers.pluck(:quote_decimals).compact.min
  end
end
