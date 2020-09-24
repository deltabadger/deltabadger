require 'result'

module ExchangeApi
  module Clients
    module Binance
      class LimitTrader < BaseTrader
        def buy(currency:, price:, percentage:)
          buy_params = get_buy_params(currency, price, percentage)
          return buy_params unless buy_params.success?

          body = buy_params.data.to_json
          place_order(body)
        end

        def sell(currency:, price:, percentage:)
          sell_params = get_sell_params(currency, price, percentage)
          return sell_params unless sell_params.success?

          body = sell_params.data.to_json
          place_order(body)
        end

        private

        def get_buy_params(currency, price, percentage)
          rate = current_ask_price(currency)
          return rate unless rate.success?

          limit_rate = rate.data * (1 - percentage / 100)
          quantity = (price / limit_rate).ceil(8)
          Result::Success.new(common_order_params(currency).merge(
                                type: 'buy',
                                quantity: quantity,
                                price: limit_rate
                              ))
        end

        def get_sell_params(currency, price, percentage)
          rate = current_bid_price(currency)
          return rate unless rate.success?

          limit_rate = rate.data * (1 + percentage / 100)
          quantity = (price / limit_rate).ceil(8)
          Result::Success.new(common_order_params(currency).merge(
                                type: 'sell',
                                quantity: quantity,
                                price: limit_rate
                              ))
        end

        def common_order_params(currency)
          super(currency).merge(side: 'limit', timeInForce: 'GTC')
        end
      end
    end
  end
end
