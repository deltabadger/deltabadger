require 'result'

module ExchangeApi
  module Traders
    module Binance
      class LimitTrader < BaseTrader
        def buy(currency:, price:, percentage:)
          final_price = transaction_price(currency, price)
          buy_params = get_buy_params(currency, final_price, percentage)
          return buy_params unless buy_params.success?

          place_order(buy_params.data)
        end

        def sell(currency:, price:, percentage:)
          final_price = transaction_price(currency, price)
          sell_params = get_sell_params(currency, final_price, percentage)
          return sell_params unless sell_params.success?

          place_order(sell_params.data)
        end

        private

        def get_buy_params(currency, price, percentage)
          rate = current_ask_price(currency)
          return rate unless rate.success?

          limit_rate = (rate.data * (1 - percentage / 100)).ceil(2)
          quantity = transaction_quantity(price, limit_rate)
          Result::Success.new(common_order_params(currency).merge(
                                side: 'BUY',
                                quantity: quantity,
                                price: limit_rate
                              ))
        end

        def get_sell_params(currency, price, percentage)
          rate = current_bid_price(currency)
          return rate unless rate.success?

          limit_rate = (rate.data * (1 + percentage / 100)).ceil(2)
          quantity = transaction_quantity(price, limit_rate)
          Result::Success.new(common_order_params(currency).merge(
                                side: 'SELL',
                                quantity: quantity,
                                price: limit_rate
                              ))
        end

        def common_order_params(currency)
          super(currency).merge(type: 'LIMIT', timeInForce: 'GTC')
        end
      end
    end
  end
end
