module Bots::Free::Validators
  class Create < BaseService
    def call(bot, user)
      allowed_symbols = bot.exchange.symbols
      return allowed_symbols unless allowed_symbols.success?

      non_hodler_symbols = bot.exchange.non_hodler_symbols
      return non_hodler_symbols unless allowed_symbols.success?

      bot_settings = BotSettings.new(bot.settings, user, allowed_symbols.data, non_hodler_symbols.data)

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

      attr_reader :interval, :base, :quote, :type, :order_type, :price,
                  :percentage, :allowed_symbols, :non_hodler_symbols, :hodler, :force

      INTERVALS = %w[month week day hour].freeze
      TYPES = %w[buy sell].freeze
      ORDER_TYPES = %w[market limit].freeze

      validates :interval, :base, :quote, :type, :order_type, :price, :force, presence: true
      validate :allowed_symbol
      validate :hodler_allowed_symbol
      validates :interval, inclusion: { in: INTERVALS }
      validates :type, inclusion: { in: TYPES }
      validates :order_type, inclusion: { in: ORDER_TYPES }
      validates :price, numericality: { only_float: true, greater_than: 0 }
      validates :percentage, allow_nil: true, numericality: {
        only_float: true,
        greater_than: 0,
        smaller_than: 100
      }
      validate :hodler_if_limit_order
      validate :percentage_if_limit_order
      validate :interval_within_limit

      def initialize(params, user, allowed_symbols, non_hodler_symbols)
        @interval = params['interval']
        @base = params['base']
        @quote = params['quote']
        @type = params['type']
        @order_type = params['order_type']
        @price = params['price']&.to_f
        @percentage = params['percentage']&.to_f
        @force = params['force']
        @allowed_symbols = allowed_symbols
        @non_hodler_symbols = non_hodler_symbols
        @hodler = user.subscription_name == 'hodler'
      end

      private

      def allowed_symbol
        symbol = ExchangeApi::Markets::MarketSymbol.new(base, quote)
        return if symbol.in?(allowed_symbols)

        errors.add(:symbol, "#{symbol} is not supported")
      end

      def interval_within_limit
        result = Bots::Free::Validators::IntervalWithinLimit.call(
          interval: interval,
          price: price,
          currency: quote
        )

        errors.add(:base, result.errors.first) if result.failure?
      end

      def hodler_allowed_symbol
        symbol = ExchangeApi::Markets::MarketSymbol.new(base, quote)
        return if hodler || symbol.in?(non_hodler_symbols)

        errors.add(:symbol, "#{symbol} is not supported in your subscription")
      end

      def hodler_if_limit_order
        return if hodler || order_type == 'market'

        errors.add(:base, 'Limit orders are an hodler-only functionality')
      end

      def percentage_if_limit_order
        return if order_type == 'market' || (order_type == 'limit' && percentage.present?)

        errors.add(:base, 'Specify percentage when creating a limit order')
      end
    end
  end
end
