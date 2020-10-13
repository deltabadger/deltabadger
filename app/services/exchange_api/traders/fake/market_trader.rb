require 'result'

module ExchangeApi
  module Traders
    module Fake
      class MarketTrader < ExchangeApi::Traders::Fake::BaseTrader
        def buy(currency:, price:)
          buy_params = get_buy_params(currency, price)
          return buy_params unless buy_params.success?

          place_order(buy_params.data)
        end

        def sell(currency:, price:)
          sell_params = get_sell_params(currency, price)
          return sell_params unless sell_params.success?

          place_order(sell_params.data)
        end

        private

        def get_buy_params(currency, price)
          rate = current_ask_price(currency)
          return rate unless rate.success?

          volume = smart_volume(price, rate.data)
          return volume unless volume.success?

          Result::Success.new(common_order_params.merge(amount: volume.data, rate: rate.data))
        end

        def get_sell_params(currency, price)
          rate = current_bid_price(currency)
          return rate unless rate.success?

          volume = smart_volume(price, rate.data)
          return volume unless volume.success?

          Result::Success.new(common_order_params.merge(amount: volume.data, rate: rate.data))
        end
      end
    end
  end
end
