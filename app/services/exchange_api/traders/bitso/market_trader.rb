module ExchangeApi
  module Traders
    module Bitso
      class MarketTrader < ExchangeApi::Traders::Bitso::BaseTrader
        def buy(base:, quote:, price:, force_smart_intervals:, smart_intervals_value:)
          buy_params = get_params(base, quote, price, force_smart_intervals, 'buy', smart_intervals_value)
          return buy_params unless buy_params.success?

          place_order(buy_params.data)
        end

        def sell(base:, quote:, price:, force_smart_intervals:, smart_intervals_value:)
          sell_params = get_params(base, quote, price, force_smart_intervals, 'sell', smart_intervals_value)
          return sell_params unless sell_params.success?

          place_order(sell_params.data)
        end

        private

        def get_params(base, quote, price, force_smart_intervals, side, smart_intervals_value)
          symbol = @market.symbol(base, quote)
          common_params = common_order_params(symbol, price, force_smart_intervals, smart_intervals_value)
          return common_params unless common_params.success?

          Result::Success.new(
            common_params.data.merge(
              side: side
            )
          )
        end

        def common_order_params(symbol, price, force_smart_intervals, smart_intervals_value)
          price_above_minimums = transaction_price(symbol, price, force_smart_intervals, smart_intervals_value)
          return price_above_minimums unless price_above_minimums.success?

          Result::Success.new(
            super(symbol).merge(
              minor: price_above_minimums.data.to_s,
              type: 'market'
            )
          )
        end
      end
    end
  end
end
