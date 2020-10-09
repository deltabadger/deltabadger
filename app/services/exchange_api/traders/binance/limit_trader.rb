require 'result'

module ExchangeApi
  module Traders
    module Binance
      class LimitTrader < ExchangeApi::Traders::Binance::BaseTrader
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

        def parse_response(response)
          return error_to_failure([response['msg']]) if response['msg'].present?

          Result::Success.new(
            offer_id: response['orderId'],
            rate: response['price'],
            amount: response['origQty'] # We treat the order as fully completed
          )
        end

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
