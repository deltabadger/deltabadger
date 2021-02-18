module Bots::Free::Validators
  class IntervalWithinLimit < BaseService
    LIMIT_CREDITS_PER_HOUR = 1000
    LIMITS = {
      hour: LIMIT_CREDITS_PER_HOUR,
      day: LIMIT_CREDITS_PER_HOUR * 24,
      week: LIMIT_CREDITS_PER_HOUR * 24 * 7,
      month: LIMIT_CREDITS_PER_HOUR * 24 * 30
    }.freeze

    def initialize(price_to_credits: ConvertCurrencyToCredits.new)
      @price_to_credits = price_to_credits
    end

    def call(interval:, price:, currency:)
      credits = @price_to_credits.call(amount: price.to_f, currency: currency)

      if credits <= LIMITS[interval.to_sym]
        Result::Success.new
      else
        Result::Failure.new(I18n.t('errors.frequency_limit'))
      end
    end
  end
end
