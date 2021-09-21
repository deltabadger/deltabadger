module ExchangeApi
  module Traders
    module Binance
      class MarketTrader < ExchangeApi::Traders::Binance::BaseTrader
        def buy(base:, quote:, price:, force_smart_intervals:, smart_intervals_value:)
          symbol = @market.symbol(base, quote)
          buy_params = get_buy_params(symbol, price, force_smart_intervals, smart_intervals_value)
          return buy_params unless buy_params.success?

          place_order(buy_params.data)
        end

        def sell(base:, quote:, price:, force_smart_intervals:, smart_intervals_value:, is_legacy:)
          symbol = @market.symbol(base, quote)
          sell_params = get_sell_params(symbol, price, force_smart_intervals, smart_intervals_value, is_legacy)
          return sell_params unless sell_params.success?

          place_order(sell_params.data)
        end

        private

        def get_buy_params(symbol, price, force_smart_intervals, smart_intervals_value)
          common_params = common_order_params(symbol, price, force_smart_intervals, smart_intervals_value, true)
          return common_params unless common_params.success?

          Result::Success.new(common_params.data.merge(side: 'BUY'))
        end

        def get_sell_params(symbol, price, force_smart_intervals, smart_intervals_value, price_in_quote)
          common_params = common_order_params(symbol, price, force_smart_intervals, smart_intervals_value, price_in_quote)
          return common_params unless common_params.success?

          Result::Success.new(common_params.data.merge(side: 'SELL'))
        end

        def common_order_params(symbol, price, force_smart_intervals, smart_intervals_value, price_in_quote)
          price_in_quote ||= force_smart_intervals # minimum is defined in quote
          price = transaction_price(symbol, price, force_smart_intervals, smart_intervals_value, price_in_quote)
          return price unless price.success?

          precision = price_in_quote ? @market.quote_tick_size_decimals(symbol) : @market.base_tick_size_decimals(symbol)
          return precision unless precision.success?

          parsed_base = parse_base(symbol, price.data).data
          if price_in_quote
            Result::Success.new(super(symbol)
                           .merge(type: 'MARKET',
                                  quoteOrderQty: price.data.ceil(precision.data)))
          else
            Result::Success.new(super(symbol).merge(type: 'MARKET', quantity: parsed_base))
          end
        end
      end
    end
  end
end
