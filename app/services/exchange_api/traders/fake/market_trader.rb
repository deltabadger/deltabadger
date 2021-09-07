require 'result'

module ExchangeApi
  module Traders
    module Fake
      class MarketTrader < ExchangeApi::Traders::Fake::BaseTrader
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

          Result::Success.new(common_order_params.merge(amount: volume.data, rate: rate.data))
        end

        def get_sell_params(symbol, price, force_smart_intervals, smart_intervals_value)
          rate = @market.current_bid_price(symbol)
          return rate unless rate.success?

          volume = smart_volume(symbol, price, rate.data, force_smart_intervals, smart_intervals_value, true )
          return volume unless volume.success?

          Result::Success.new(common_order_params.merge(amount: volume.data, rate: rate.data))
        end
      end
    end
  end
end
