module Bots::Free::Validators
  class Create < BaseService
    def call(bot)
      allowed_currencies = bot.exchange.currencies
      bot_settings = BotSettings.new(bot.settings, allowed_currencies)

      if bot.valid? && bot_settings.valid?
        Result::Success.new
      else
        Result::Failure.new(
          *(bot.errors.full_messages + bot_settings.errors.full_messages)
        )
      end
    end

    class BotSettings
      include ActiveModel::Validations

      attr_reader :interval, :currency, :type, :price, :allowed_currencies

      INTERVALS = %w[month week day hour].freeze
      TYPES = %w[buy sell].freeze

      validates :interval, :currency, :type, :price, presence: true
      validate :allowed_currency
      validates :interval, inclusion: { in: INTERVALS }
      validates :type, inclusion: { in: TYPES }
      validates :price, numericality: { only_float: true, greater_than: 0 }
      validate :interval_within_limit

      def initialize(params, allowed_currencies)
        @interval = params.fetch('interval')
        @currency = params.fetch('currency')
        @type = params.fetch('type')
        @price = params.fetch('price').to_f
        @allowed_currencies = allowed_currencies
      end

      private

      def allowed_currency
        return if currency.in?(allowed_currencies)

        errors.add(:currency, "'#{currency}' is not allowed")
      end

      def interval_within_limit
        result = Bots::Free::Validators::IntervalWithinLimit.call(
          interval: interval,
          price: price,
          currency: currency
        )

        errors.add(:base, result.errors.first) if result.failure?
      end
    end
  end
end
