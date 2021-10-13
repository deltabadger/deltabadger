module Bots::Free::Validators
  class Create < BaseService
    def call(bot, user)
      allowed_symbols = bot.exchange.symbols
      return allowed_symbols unless allowed_symbols.success?

      free_plan_symbols = bot.exchange.free_plan_symbols
      return free_plan_symbols unless free_plan_symbols.success?

      exchange_name = Exchange.find(bot.exchange_id).name.downcase

      bot_settings = BotSettings.new(bot.settings, user,
                                     allowed_symbols.data, free_plan_symbols.data, exchange_name)

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
                  :percentage, :allowed_symbols, :free_plan_symbols,
                  :hodler, :force_smart_intervals, :smart_intervals_value, :exchange_name,
                  :price_range_enabled, :price_range

      INTERVALS = %w[month week day hour].freeze
      TYPES = %w[buy sell sell_old].freeze
      ORDER_TYPES = %w[market limit].freeze

      validates :interval, :base, :quote, :type, :order_type, :price, presence: true
      validate :allowed_symbol
      validate :plan_allowed_symbol
      validates :interval, inclusion: { in: INTERVALS }
      validates :type, inclusion: { in: TYPES }
      validates :order_type, inclusion: { in: ORDER_TYPES }
      validates :price, numericality: { only_float: true, greater_than: 0 }
      validates :force_smart_intervals, inclusion: { in: [true, false] }
      validates :smart_intervals_value, numericality: { only_float: true, greater_than: 0 }
      validates :price_range_enabled, inclusion: { in: [true, false] }
      validates :percentage, allow_nil: true, numericality: {
        only_float: true,
        greater_than_or_equal_to: 0,
        smaller_than: 100
      }
      validate :hodler_if_limit_order
      validate :percentage_if_limit_order
      validate :smart_intervals_above_minimum
      validate :hodler_if_price_range
      validate :validate_price_range

      def initialize(params, user, allowed_symbols, free_plan_symbols, exchange_name)
        @interval = params['interval']
        @base = params['base']
        @quote = params['quote']
        @type = params['type']
        @order_type = params['order_type']
        @price = params['price']&.to_f
        @percentage = params['percentage']&.to_f
        @force_smart_intervals = params['force_smart_intervals']
        @smart_intervals_value = params['smart_intervals_value']
        @price_range_enabled = params['price_range_enabled']
        @price_range = params['price_range']
        @allowed_symbols = allowed_symbols
        @free_plan_symbols = free_plan_symbols
        @exchange_name = exchange_name
        @hodler = user.subscription_name == 'hodler'
        @paid_plan = user.subscription_name == 'hodler' || user.subscription_name == 'investor'
        @minimums = GetSmartIntervalsInfo.new.call(params.merge(exchange_name: exchange_name), user).data
      end

      private

      def allowed_symbol
        symbol = ExchangeApi::Markets::MarketSymbol.new(base, quote)
        return if symbol.in?(allowed_symbols)

        errors.add(:symbol, "#{symbol} is not supported")
      end

      def plan_allowed_symbol
        symbol = ExchangeApi::Markets::MarketSymbol.new(base, quote)
        return if @paid_plan || symbol.in?(free_plan_symbols)

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

      def smart_intervals_above_minimum
        return unless @force_smart_intervals

        @minimum = if limit_minimum_in_base?(@exchange_name, order_type)
                     @minimums[:minimum_limit].to_f
                   else
                     @minimums[:minimum].to_f
                   end

        return if @smart_intervals_value.to_f >= @minimum.to_f

        errors.add(:smart_intervals_value, " should be greater than #{@minimum}")
      end

      def validate_price_range
        return if !@price_range_enabled || price_range_valid?

        errors.add(:price_range, ' is invalid')
      end

      def price_range_valid?
        @price_range.length == 2 &&
          @price_range[0].to_f >= 0 &&
          @price_range[1].to_f >= @price_range[0].to_f
      end

      def limit_minimum_in_base?(exchange_name, order_type)
        order_type == 'limit' && ['coinbase pro', 'kucoin'].include?(exchange_name.downcase)
      end

      def hodler_if_price_range
        return if hodler || !@price_range_enabled

        errors.add(:base, 'Price range is an hodler-only functionality')
      end
    end
  end
end
