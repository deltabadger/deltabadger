module Bots::Webhook::Validators
  class Create < BaseService
    def call(bot, user)
      allowed_symbols = bot.exchange.symbols
      return allowed_symbols unless allowed_symbols.success?

      free_plan_symbols = bot.exchange.free_plan_symbols
      return free_plan_symbols unless free_plan_symbols.success?

      exchange_name = Exchange.find(bot.exchange_id).name.downcase

      bot_settings = BotSettings.new(bot.settings, user,
                                     allowed_symbols.data, free_plan_symbols.data, exchange_name, bot.exchange_id)

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

      attr_reader :base, :quote, :type, :order_type, :price, :allowed_symbols, :free_plan_symbols,
                  :hodler, :legendary_badger, :exchange_name, :name, :trigger_url, :trigger_possibility,
                  :already_triggered_types, :additional_type, :additional_type_enabled,
                  :additional_trigger_url, :additional_price

      BUY_TYPES = %w[buy buy_all].freeze
      SELL_TYPES = %w[sell sell_all].freeze
      TYPES = (BUY_TYPES + SELL_TYPES).freeze
      ORDER_TYPES = %w[market limit].freeze

      validates :type, :price, :base, :quote, :name, :trigger_url, :trigger_possibility, :order_type, presence: true
      validates :additional_type, :additional_trigger_url, :additional_price, presence: true, if: -> { additional_type_enabled }
      validate :allowed_symbol
      validate :plan_allowed_symbol
      validates :type, inclusion: { in: TYPES }
      validates :additional_type, inclusion: { in: TYPES }, if: -> { additional_type_enabled }
      validate :allowed_additional_type

      validates :order_type, inclusion: { in: ORDER_TYPES }
      validates :price, numericality: { only_float: true, greater_than: 0 }
      validates :additional_price, numericality: { only_float: true, greater_than: 0 }, if: -> { additional_type_enabled }

      def initialize(params, user, allowed_symbols, free_plan_symbols, exchange_name, exchange_id)
        @base = params['base']
        @quote = params['quote']
        @type = params['type']
        @order_type = params['order_type']
        @price = params['price']&.to_f
        @name = params['name']
        @trigger_url = params['trigger_url']
        @additional_type_enabled = params['additional_type_enabled']
        @additional_type = params['additional_type']
        @additional_trigger_url = params['additional_trigger_url']
        @additional_price = params['additional_price']&.to_f
        @trigger_possibility = params['trigger_possibility']
        @already_triggered_types = params['already_triggered_types']
        @allowed_symbols = allowed_symbols
        @free_plan_symbols = free_plan_symbols
        @exchange_name = exchange_name
        @hodler = user.subscription_name == 'hodler'
        @legendary_badger = user.subscription_name == 'legendary_badger'
        @paid_plan = user.subscription_name == 'hodler' || user.subscription_name == 'investor' || user.subscription_name == 'legendary_badger'
        @minimums = GetSmartIntervalsInfo.new.call(params.merge(exchange_name: exchange_name), user).data

        @exchange_id = exchange_id
        @user = user
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

      def hodler_or_legendary_badger_if_limit_order
        return if hodler || legendary_badger || order_type == 'market'

        errors.add(:base, 'Limit orders are an hodler and legendary_badger only functionality')
      end

      def allowed_additional_type
        errors.add(:additional_type, "must be one of #{SELL_TYPES}") if type.in?(BUY_TYPES) && additional_type.in?(BUY_TYPES)
        errors.add(:additional_type, "must be one of #{BUY_TYPES}") if type.in?(SELL_TYPES) && additional_type.in?(SELL_TYPES)
      end
    end
  end
end
