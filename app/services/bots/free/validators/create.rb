module Bots::Free::Validators
  class Create < BaseService
    def call(bot, user)
      allowed_currencies = bot.exchange.currencies
      bot_settings = BotSettings.new(bot.settings, user, allowed_currencies)

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

      attr_reader :interval, :currency, :type, :order_type, :price,
                  :percentage, :allowed_currencies, :admin

      INTERVALS = %w[month week day hour].freeze
      TYPES = %w[buy sell].freeze
      ORDER_TYPES = %w[market limit].freeze

      validates :interval, :currency, :type, :order_type, :price, presence: true
      validate :allowed_currency
      validates :interval, inclusion: { in: INTERVALS }
      validates :type, inclusion: { in: TYPES }
      validates :order_type, inclusion: { in: ORDER_TYPES }
      validates :price, numericality: { only_float: true, greater_than: 0 }
      validates :percentage, allow_nil: true, numericality: {
        only_float: true,
        greater_than: 0,
        smaller_than: 100
      }
      validate :admin_if_limit_order
      validate :percentage_if_limit_order
      validate :interval_within_limit

      def initialize(params, user, allowed_currencies)
        @interval = params.fetch('interval')
        @currency = params.fetch('currency')
        @type = params.fetch('type')
        @order_type = params.fetch('order_type')
        @price = params.fetch('price').to_f
        @percentage = params.fetch('percentage', nil)&.to_f
        @allowed_currencies = allowed_currencies
        @admin = user.admin
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

      def admin_if_limit_order
        return if admin || order_type == 'market'

        errors.add(:base, 'Limit orders are an admin-only functionality')
      end

      def percentage_if_limit_order
        return if order_type == 'market' || (order_type == 'limit' && percentage.present?)

        errors.add(:base, 'Specify percentage when creating a limit order')
      end
    end
  end
end
