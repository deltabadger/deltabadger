require 'result'

module ExchangeApi
  module Traders
    module Binance
      class LimitTrader < ExchangeApi::Traders::Binance::BaseTrader
        def buy(base:, quote:, price:, percentage:)
          symbol = @market.symbol(base, quote)
          final_price = transaction_price(symbol, price)
          buy_params = get_buy_params(symbol, final_price, percentage)
          return buy_params unless buy_params.success?

          place_order(buy_params.data)
        end

        def sell(base:, quote:, price:, percentage:)
          symbol = @market.symbol(base, quote)
          final_price = transaction_price(symbol, price)
          sell_params = get_sell_params(symbol, final_price, percentage)
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

        def get_buy_params(symbol, price, percentage)
          rate = @market.current_ask_price(symbol)
          return rate unless rate.success?

          quote_decimals = @market.quote_decimals(symbol)
          return quote_decimals unless quote_decimals.success?

          limit_rate = (rate.data * (1 - percentage / 100)).ceil(quote_decimals.data)
          quantity = transaction_volume(price, limit_rate)
          Result::Success.new(common_order_params(symbol).merge(
                                side: 'BUY',
                                quantity: quantity,
                                price: limit_rate
                              ))
        end

        def get_sell_params(symbol, price, percentage)
          rate = @market.current_bid_price(symbol)
          return rate unless rate.success?

          quote_decimals = @market.quote_decimals(symbol)
          return quote_decimals unless quote_decimals.success?

          limit_rate = (rate.data * (1 + percentage / 100)).ceil(quote_decimals.data)
          quantity = transaction_volume(price, limit_rate)
          Result::Success.new(common_order_params(symbol).merge(
                                side: 'SELL',
                                quantity: quantity,
                                price: limit_rate
                              ))
        end

        def common_order_params(symbol)
          super(symbol).merge(type: 'LIMIT', timeInForce: 'GTC')
        end
      end
    end
  end
end
