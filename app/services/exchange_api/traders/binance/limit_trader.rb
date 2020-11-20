require 'result'

module ExchangeApi
  module Traders
    module Binance
      class LimitTrader < ExchangeApi::Traders::Binance::BaseTrader
        def buy(base:, quote:, price:, percentage:, force_smart_intervals:)
          symbol = @market.symbol(base, quote)
          final_price = transaction_price(symbol, price, force_smart_intervals)
          return final_price unless final_price.success?

          buy_params = get_buy_params(symbol, final_price.data, percentage)
          return buy_params unless buy_params.success?

          place_order(buy_params.data)
        end

        def sell(base:, quote:, price:, percentage:, force_smart_intervals:)
          symbol = @market.symbol(base, quote)
          final_price = transaction_price(symbol, price, force_smart_intervals)
          return final_price unless final_price.success?

          sell_params = get_sell_params(symbol, final_price.data, percentage)
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
          rate = limit_rate(symbol, percentage)
          return rate unless rate.success?

          quantity = transaction_volume(symbol, price, rate.data)
          return quantity unless quantity.success?

          Result::Success.new(common_order_params(symbol).merge(
                                side: 'BUY',
                                quantity: quantity.data,
                                price: rate.data
                              ))
        end

        def get_sell_params(symbol, price, percentage)
          rate = limit_rate(symbol, percentage)
          return rate unless rate.success?

          quantity = transaction_volume(symbol, price, rate.data)
          return quantity unless quantity.success?

          Result::Success.new(common_order_params(symbol).merge(
                                side: 'SELL',
                                quantity: quantity.data,
                                price: rate.data
                              ))
        end

        def limit_rate(symbol, percentage)
          rate = @market.current_ask_price(symbol)
          return rate unless rate.success?

          quote_tick = @market.quote_tick_size(symbol)
          return quote_tick unless quote_tick.success?

          percentage_rate = rate.data * (1 - percentage / 100)
          ceil_to_min_tick = (percentage_rate / quote_tick.data).ceil * quote_tick.data
          Result::Success.new(ceil_to_min_tick)
        end

        def common_order_params(symbol)
          super(symbol).merge(type: 'LIMIT', timeInForce: 'GTC')
        end
      end
    end
  end
end
