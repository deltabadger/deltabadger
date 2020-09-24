require 'result'

module ExchangeApi
  module Clients
    module Bitbay
      class LimitTrader < BaseTrader
        def buy(currency:, price:, percentage:)
          buy_params = get_buy_params(price, percentage)
          return buy_params unless buy_params.success?

          place_order(currency, buy_params.data.to_json)
        end

        def sell(currency:, price:, percentage:)
          sell_params = get_sell_params(price, percentage)
          return sell_params unless sell_params.success?

          place_order(currency, sell_params.data.to_json)
        end

        private

        def get_buy_params(price, percentage)
          rate = current_ask_price(currency)
          return rate unless rate.success?

          limit_rate = rate.data * (1 - percentage / 100)
          Result::Success.new(common_order_params(price).merge(offerType: 'buy', rate: limit_rate))
        end

        def get_sell_params(price, percentage)
          rate = current_bid_price(currency)
          return rate unless rate.success?

          limit_rate = rate.data * (1 + percentage / 100)
          Result::Success.new(common_order_params(price).merge(offerType: 'sell', rate: limit_rate))
        end

        def common_order_params(price)
          super(price).merge(mode: 'limit')
        end
      end
    end
  end
end
