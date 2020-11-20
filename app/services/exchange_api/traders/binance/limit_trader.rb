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
          common_params = common_order_params(symbol, price, -percentage)

          Result::Success.new(common_params.data.merge(side: 'BUY'))
        end

        def get_sell_params(symbol, price, percentage)
          common_params = common_order_params(symbol, price, percentage)
          return common_params unless common_params.success?

          Result::Success.new(common_params.data.merge(side: 'SELL'))
        end

        def common_order_params(symbol, price, percentage)
          rate = limit_rate(symbol, percentage)
          return rate unless rate.success?

          quantity = transaction_volume(symbol, price, rate.data)
          return quantity unless quantity.success?

          parsed_quantity = parse_base(symbol, quantity.data)
          return parsed_quantity unless parsed_quantity.success?

          parsed_price = parse_quote(symbol, rate.data)
          return parsed_price unless parsed_price.success?

          Result::Success.new(super(symbol).merge(
                                type: 'LIMIT',
                                timeInForce: 'GTC',
                                quantity: parsed_quantity.data,
                                price: parsed_price.data
                              ))
        end

        def limit_rate(symbol, percentage)
          rate = @market.current_ask_price(symbol)
          return rate unless rate.success?

          quote_tick = @market.quote_tick_size(symbol)
          return quote_tick unless quote_tick.success?

          quote_decimals = @market.quote_decimals(symbol)
          return quote_decimals unless quote_decimals.success?

          percentage_rate = rate.data * (1 + percentage / 100)
          ceil_to_min_tick = (
            (percentage_rate / quote_tick.data).ceil * quote_tick.data
          ).ceil(quote_decimals.data)
          Result::Success.new(ceil_to_min_tick)
        end
      end
    end
  end
end
