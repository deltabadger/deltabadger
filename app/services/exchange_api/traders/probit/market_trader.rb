module ExchangeApi
  module Traders
    module Probit
      class MarketTrader < ExchangeApi::Traders::Probit::BaseTrader
        def buy(base:, quote:, price:, force_smart_intervals:, smart_intervals_value:)
          params = get_params('buy', price, base, quote, force_smart_intervals, smart_intervals_value)
          return params unless params.success?

          place_order(params.data)
        end

        def sell(base:, quote:, price:, force_smart_intervals:, smart_intervals_value:, is_legacy:)
          params = get_params('sell', price, base, quote, force_smart_intervals, smart_intervals_value)
          return params unless params.success?

          place_order(params.data)
        end

        def fetch_order_by_id(order_id, result_params)
          response = super
          return response unless response.success?

          response_data = response.data['data'][0]
          return Result::Failure.new('Order cancelled by Probit') if response_data['status'] == 'cancelled'

          rate = response_data['filled_cost'].to_f / response_data['filled_quantity']
          Result::Success.new(
            offer_id: order_id,
            amount: response_data['filled_quantity'].to_s,
            rate: rate
          )
        end

        private

        def get_params(side, price, base, quote, force_smart_intervals, smart_intervals_value)
          symbol = @market.symbol(base, quote)
          if side == 'buy'
            cost = transaction_cost(price, symbol, force_smart_intervals, smart_intervals_value)
            return cost unless cost.success?

            Result::Success.new(common_params(symbol).data.merge(
                                  "cost": cost.data.to_s,
                                  "side": side
                                ))
          else
            quantity = transaction_quantity(price, symbol, force_smart_intervals, smart_intervals_value)
            return quantity unless quantity.success?

            Result::Success.new(common_params(symbol).data.merge(
                                  "quantity": quantity.data.to_s,
                                  "side": side
                                ))
          end
        end

        def common_params(symbol)
          Result::Success.new("market_id": symbol,
                              "time_in_force": 'ioc',
                              "type": 'market')
        end
      end
    end
  end
end
