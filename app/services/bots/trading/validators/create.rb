module Bots::Trading::Validators
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

      attr_reader :interval, :base, :quote, :type, :order_type, :price,
                  :percentage, :allowed_symbols, :free_plan_symbols,
                  :pro, :legendary, :force_smart_intervals, :smart_intervals_value, :exchange_name,
                  :price_range_enabled, :price_range, :use_subaccount, :selected_subaccount

      INTERVALS = %w[month week day hour].freeze
      TYPES = %w[buy sell sell_old].freeze
      ORDER_TYPES = %w[market limit].freeze

      validates :interval, :base, :quote, :type, :order_type, :price, presence: true
      validate :allowed_symbol
      validate :plan_allowed_symbol
      validate :plan_allowed_bot
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
      validates :use_subaccount, inclusion: { in: [true, false, nil] }
      validate :pro_or_legendary_if_limit_order
      validate :percentage_if_limit_order
      validate :smart_intervals_above_minimum
      validate :pro_or_legendary_if_price_range
      validate :validate_price_range
      validate :validate_use_subaccount
      validate :validate_subaccount_name

      def initialize(params, user, allowed_symbols, free_plan_symbols, exchange_name, exchange_id)
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
        @pro = user.subscription.pro?
        @legendary = user.subscription.legendary?
        @paid_plan = user.subscription.paid?
        @minimums = GetSmartIntervalsInfo.new.call(params.merge(exchange_name: exchange_name), user).data
        @use_subaccount = params['use_subaccount']
        @selected_subaccount = params['selected_subaccount']
        @exchange_id = exchange_id
        @user = user
      end

      private

      def allowed_symbol
        symbol = ExchangeApi::Markets::MarketSymbol.new(base, quote)
        return if symbol.in?(allowed_symbols)

        errors.add(:symbol, I18n.t('bots.messages.symbol_not_supported', symbol: symbol))
      end

      def plan_allowed_symbol
        symbol = ExchangeApi::Markets::MarketSymbol.new(base, quote)
        return if @paid_plan || symbol.in?(free_plan_symbols)

        errors.add(:symbol, I18n.t('bots.messages.symbol_not_supported_subscription', symbol: symbol))
      end

      def plan_allowed_bot
        return if @user.unlimited? || @user.bots.working.count.zero?

        errors.add(:base, I18n.t('bots.messages.upgrade_plan_more_bots'))
      end

      def pro_or_legendary_if_limit_order
        return if pro || legendary || order_type == 'market'

        errors.add(:base, I18n.t('bots.messages.upgrade_plan_limit_orders'))
      end

      def percentage_if_limit_order
        return if order_type == 'market' || (order_type == 'limit' && percentage.present?)

        errors.add(:base, I18n.t('bots.messages.specify_percentage_limit_order'))
      end

      def smart_intervals_above_minimum
        return unless @force_smart_intervals

        @minimum = if limit_minimum_in_base?(@exchange_name, order_type)
                     @minimums[:minimum_limit].to_f
                   else
                     @minimums[:minimum].to_f
                   end

        return if @smart_intervals_value.to_f >= @minimum.to_f

        errors.add(:smart_intervals_value, I18n.t('bots.messages.smart_intervals_above_minimum', minimum: @minimum))
      end

      def validate_price_range
        return if !@price_range_enabled || price_range_valid?

        errors.add(:price_range, I18n.t('bots.messages.invalid_price_range'))
      end

      def price_range_valid?
        @price_range.length == 2 &&
          @price_range[0].to_f >= 0 &&
          @price_range[1].to_f >= @price_range[0].to_f
      end

      def limit_minimum_in_base?(exchange_name, order_type)
        order_type == 'limit' && ['coinbase pro', 'kucoin'].include?(exchange_name.downcase)
      end

      def pro_or_legendary_if_price_range
        return if pro || legendary || !@price_range_enabled

        errors.add(:base, I18n.t('bots.messages.upgrade_price_range'))
      end

      def validate_use_subaccount
        return unless @use_subaccount && !subaccounts_allowed_exchange

        errors.add(:use_subaccount, I18n.t('bots.messages.subaccounts_not_allowed'))
      end

      def validate_subaccount_name
        return unless @use_subaccount

        if @selected_subaccount.nil? || @selected_subaccount.empty?
          errors.add(:selected_subaccount, I18n.t('bots.messages.no_subaccount_name'))
          return
        end

        market = ExchangeApi::Markets::Get.call(@exchange_id)
        subaccounts = market.subaccounts(get_api_keys(@user, @exchange_id))

        return if (subaccounts.success? && subaccounts.data.include?(@selected_subaccount)) || subaccounts.failure?

        errors.add(:selected_subaccount, I18n.t('bots.messages.wrong_subaccount_name'))
      end

      def get_api_keys(user, exchange_id)
        ApiKey.find_by(user: user, exchange_id: exchange_id, key_type: 'trading')
      end

      def subaccounts_allowed_exchange
        ['ftx', 'ftx.us'].include?(@exchange_name)
      end
    end
  end
end
