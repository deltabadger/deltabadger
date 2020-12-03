require 'result'

module ExchangeApi
  module Traders
    module Kraken
      class LimitTrader < ExchangeApi::Traders::Kraken::BaseTrader
        def buy(base:, quote:, price:, percentage:, force_smart_intervals:)
          symbol = @market.symbol(base, quote)
          buy_params = get_buy_params(symbol, price, percentage, force_smart_intervals)
          return buy_params unless buy_params.success?

          place_order(buy_params.data)
        end

        def sell(base:, quote:, price:, percentage:, force_smart_intervals:)
          symbol = @market.symbol(base, quote)
          sell_params = get_sell_params(symbol, price, percentage, force_smart_intervals)
          return sell_params unless sell_params.success?

          place_order(sell_params.data)
        end

        private

        def orders
          open_orders = @client.open_orders.dig('result', 'open')
          closed_orders = @client.closed_orders.dig('result', 'closed') # In case a limit order gets fulfilled automatically
          open_orders.merge(closed_orders)
        end

        def placed_order_rate(order_data)
          if order_data.fetch('status') == 'open'
            order_data.fetch('descr').fetch('price').to_f
          else # closed
            super
          end
        end

        def get_buy_params(symbol, price, percentage, force_smart_intervals)
          rate = @market.current_ask_price(symbol)
          return rate unless rate.success?

          limit_rate = rate_percentage(symbol, rate.data, -percentage)
          return limit_rate unless limit_rate.success?

          volume = smart_volume(symbol, price, limit_rate.data, force_smart_intervals)
          return volume unless volume.success?

          Result::Success.new(common_order_params(symbol).merge(
                                type: 'buy',
                                volume: volume.data,
                                price: limit_rate.data
                              ))
        end

        def get_sell_params(symbol, price, percentage, force_smart_intervals)
          rate = @market.current_bid_price(symbol)
          return rate unless rate.success?

          limit_rate = rate_percentage(symbol, rate.data, percentage)
          return limit_rate unless limit_rate.success?

          volume = smart_volume(symbol, price, limit_rate.data, force_smart_intervals)
          return volume unless volume.success?

          Result::Success.new(common_order_params(symbol).merge(
                                type: 'sell',
                                volume: volume.data,
                                price: limit_rate.data
                              ))
        end

        def rate_percentage(symbol, rate, percentage)
          rate_decimals = @market.quote_decimals(symbol)
          return rate_decimals unless rate_decimals.success?

          Result::Success.new((rate * (1 + percentage / 100)).ceil(rate_decimals.data))
        end

        def common_order_params(symbol)
          super(symbol).merge(ordertype: 'limit')
        end
      end
    end
  end
end
