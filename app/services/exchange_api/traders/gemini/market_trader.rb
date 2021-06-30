module ExchangeApi
  module Traders
    module Gemini
      class MarketTrader < ExchangeApi::Traders::Gemini::BaseTrader
        def buy(base:, quote:, price:, force_smart_intervals:, smart_intervals_value:)
          symbol = @market.symbol(base, quote)
          buy_params = get_buy_params(symbol, price, force_smart_intervals, smart_intervals_value)
          return buy_params unless buy_params.success?

          place_order(buy_params.data)
        end

        def sell(base:, quote:, price:, force_smart_intervals:, smart_intervals_value:)
          symbol = @market.symbol(base, quote)
          sell_params = get_sell_params(symbol, price, force_smart_intervals, smart_intervals_value)
          return sell_params unless sell_params.success?

          place_order(sell_params.data)
        end

        private

        def get_buy_params(symbol, price, force_smart_intervals, smart_intervals_value)
          rate = @market.current_ask_price(symbol)
          return rate unless rate.success?

          volume = smart_volume(symbol, price, rate.data, force_smart_intervals, smart_intervals_value)
          return volume unless volume.success?

          price = rate_percentage(symbol, rate.data, 0.5)
          return price unless price.success?

          Result::Success.new(
            common_order_params(symbol).merge(
              side: 'buy',
              amount: volume.data,
              price: price.data
            )
          )
        end

        def get_sell_params(symbol, price, force_smart_intervals, smart_intervals_value)
          rate = @market.current_bid_price(symbol)
          return rate unless rate.success?

          volume = smart_volume(symbol, price, rate.data, force_smart_intervals, smart_intervals_value)
          return volume unless volume.success?

          price = rate_percentage(symbol, rate.data, -0.5)
          return price unless price.success?

          Result::Success.new(
            common_order_params(symbol).merge(
              side: 'sell',
              amount: volume.data,
              price: price.data
            )
          )
        end

        def rate_percentage(symbol, rate, percentage)
          rate_decimals = @market.quote_decimals(symbol)
          return rate_decimals unless rate_decimals.success?

          Result::Success.new((rate * (1 + percentage / 100)).ceil(rate_decimals.data))
        end

        def common_order_params(symbol)
          super.merge(options: ['fill-or-kill'], type: 'exchange limit')
        end
      end
    end
  end
end
