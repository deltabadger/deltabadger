require 'result'

module ExchangeApi
  module Traders
    module Kraken
      class MarketTrader < ExchangeApi::Traders::Kraken::BaseTrader
        def buy(base:, quote:, price:, force_smart_intervals:)
          symbol = @market.symbol(base, quote)
          buy_params = get_buy_params(symbol, price, force_smart_intervals)
          return buy_params unless buy_params.success?

          place_order(buy_params.data)
        end

        def sell(base:, quote:, price:, force_smart_intervals:)
          symbol = @market.symbol(base, quote)
          sell_params = get_sell_params(symbol, price, force_smart_intervals)
          return sell_params unless sell_params.success?

          place_order(sell_params.data)
        end

        private

        def parse_response(response)
          sleep(1.0)
          super
        end

        def orders
          @client.closed_orders.dig('result', 'closed')
        end

        def get_buy_params(symbol, price, force_smart_intervals)
          rate = @market.current_ask_price(symbol)
          return rate unless rate.success?

          volume = smart_volume(symbol, price, rate.data, force_smart_intervals)
          return volume unless volume.success?

          Result::Success.new(common_order_params(symbol).merge(type: 'buy', volume: volume.data))
        end

        def get_sell_params(symbol, price, force_smart_intervals)
          rate = @market.current_bid_price(symbol)
          return rate unless rate.success?

          volume = smart_volume(symbol, price, rate.data, force_smart_intervals)
          return volume unless volume.success?

          Result::Success.new(common_order_params(symbol).merge(type: 'sell', volume: volume.data))
        end

        def common_order_params(symbol)
          super(symbol).merge(ordertype: 'market')
        end
      end
    end
  end
end
