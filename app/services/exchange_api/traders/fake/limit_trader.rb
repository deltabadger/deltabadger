require 'result'

module ExchangeApi
  module Traders
    module Fake
      class LimitTrader < ExchangeApi::Traders::Fake::BaseTrader
        def buy(base:, quote:, price:, percentage:)
          symbol = @market.symbol(base, quote)
          buy_params = get_buy_params(symbol, price, percentage)
          return buy_params unless buy_params.success?

          place_order(buy_params.data)
        end

        def sell(base:, quote:, price:, percentage:)
          symbol = @market.symbol(base, quote)
          sell_params = get_sell_params(symbol, price, percentage)
          return sell_params unless sell_params.success?

          place_order(sell_params.data)
        end

        private

        def get_buy_params(symbol, price, percentage)
          rate = @market.current_ask_price(symbol)
          return rate unless rate.success?

          limit_rate = rate.data * (1 - percentage / 100)
          volume = smart_volume(symbol, price, limit_rate)
          return volume unless volume.success?

          Result::Success.new(common_order_params.merge(amount: volume.data, rate: limit_rate))
        end

        def get_sell_params(symbol, price, percentage)
          rate = @market.current_bid_price(symbol)
          return rate unless rate.success?

          limit_rate = rate.data * (1 + percentage / 100)
          volume = smart_volume(symbol, price, limit_rate)
          return volume unless volume.success?

          Result::Success.new(common_order_params.merge(amount: volume.data, rate: limit_rate))
        end
      end
    end
  end
end
