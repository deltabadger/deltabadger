module ExchangeApi
  module Traders
    module Probit
      class LimitTrader < ExchangeApi::Traders::Probit::BaseTrader
        def buy(base:, quote:, price:, percentage:, force_smart_intervals:, smart_intervals_value:)
          params = get_params('buy', price, base, quote, percentage, force_smart_intervals, smart_intervals_value)
          return params unless params.success?

          place_order(params.data)
        end

        def sell(base:, quote:, price:, percentage:, force_smart_intervals:, smart_intervals_value:, is_legacy:)
          params = get_params('sell', price, base, quote, percentage, force_smart_intervals, smart_intervals_value)
          return params unless params.success?

          place_order(params.data)
        end

        def fetch_order_by_id(order_id, result_params)
          response = super
          return response unless response.success?

          response_data = response.data['data'][0]
          return Result::Failure.new('Order cancelled by Probit') if response_data['status'] == 'cancelled'

          return Result::Failure.new('Waiting for Probit response', **NOT_FETCHED) if response_data['status'] == 'open' || response_data['filled_quantity'].zero?

          Result::Success.new(
            offer_id: order_id,
            amount: response_data['quantity'],
            rate: response_data['limit_price'].to_s
          )
        end

        private

        def get_params(side, price, base, quote, percentage, force_smart_intervals, smart_intervals_value)
          symbol = @market.symbol(base, quote)
          quantity = transaction_quantity(price, symbol, force_smart_intervals, smart_intervals_value, side == 'buy')
          return quantity unless quantity.success?

          if side == 'buy'
            rate = @market.current_ask_price(symbol).data
            rate_percentage = rate_percentage(symbol, rate, -percentage)
          else
            rate = @market.current_bid_price(symbol).data
            rate_percentage = rate_percentage(symbol, rate, percentage)
          end
          Result::Success.new(
            common_params(symbol)
              .data
              .merge(
                "quantity": quantity.data.to_s,
                "side": side,
                "limit_price": rate_percentage.data.to_s
              )
          )
        end

        def place_order(order_params)
          response = super
          return response unless response.success?

          Result::Success.new(response.data.merge(rate: order_params[:limit_price], amount: order_params[:quantity]))
        rescue StandardError
          Result::Failure.new(['Could not make Probit order', **RECOVERABLE])
        end

        def rate_percentage(symbol, rate, percentage)
          rate_decimals = @market.rate_decimals(symbol)
          return rate_decimals unless rate_decimals.success?

          Result::Success.new((rate * (1 + percentage / 100)).ceil(rate_decimals.data))
        end

        def common_params(symbol)
          Result::Success.new("market_id": symbol,
                              "time_in_force": 'gtc',
                              "type": 'limit')
        end
      end
    end
  end
end
