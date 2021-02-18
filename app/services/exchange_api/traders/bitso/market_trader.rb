module ExchangeApi
  module Traders
    module Bitso
      class MarketTrader < ExchangeApi::Traders::Bitso::BaseTrader
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

        def get_buy_params(symbol, price, force_smart_intervals)
          quote_decimals = @market.quote_decimals(symbol)
          return quote_decimals unless quote_decimals.success?

          price_above_minimums = transaction_price(symbol, price, force_smart_intervals)
          return price_above_minimums unless price_above_minimums.success?

          Result::Success.new(
            common_order_params(symbol).merge(
              minor: price_above_minimums.data.to_s,
              side: 'buy'
            )
          )
        end

        def get_sell_params(symbol, price, force_smart_intervals)
          quote_decimals = @market.quote_decimals(symbol)
          return quote_decimals unless quote_decimals.success?

          price_above_minimums = transaction_price(symbol, price, force_smart_intervals)
          return price_above_minimums unless price_above_minimums.success?

          Result::Success.new(
            common_order_params(symbol).merge(
              minor: price_above_minimums.data.to_s,
              side: 'sell'
            )
          )
        end

        def common_order_params(symbol)
          super.merge(type: 'market')
        end
      end
    end
  end
end
