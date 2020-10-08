require 'result'

module ExchangeApi
  module Traders
    module Fake
      class LimitTrader < BaseTrader
        def buy(currency:, price:, percentage:)
          buy_params = get_buy_params(currency, price, percentage)
          return buy_params unless buy_params.success?

          place_order(buy_params.data)
        end

        def sell(currency:, price:, percentage:)
          sell_params = get_sell_params(currency, price, percentage)
          return sell_params unless sell_params.success?

          place_order(sell_params.data)
        end

        private

        def get_buy_params(currency, price, percentage)
          rate = current_ask_price(currency)
          return rate unless rate.success?

          limit_rate = rate.data * (1 - percentage / 100)
          volume = smart_volume(price, limit_rate)
          return volume unless volume.success?

          Result::Success.new(common_order_params.merge(amount: volume.data, rate: limit_rate))
        end

        def get_sell_params(currency, price, percentage)
          rate = current_bid_price(currency)
          return rate unless rate.success?

          limit_rate = rate.data * (1 + percentage / 100)
          volume = smart_volume(price, limit_rate)
          return volume unless volume.success?

          Result::Success.new(common_order_params.merge(amount: volume.data, rate: limit_rate))
        end
      end
    end
  end
end
