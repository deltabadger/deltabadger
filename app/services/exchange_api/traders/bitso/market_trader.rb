module ExchangeApi
  module Traders
    module Bitso
      class MarketTrader < ExchangeApi::Traders::Bitso::BaseTrader
        def buy(base:, quote:, price:, force_smart_intervals:, smart_intervals_value:)
          buy_params = get_params(base, quote, price, force_smart_intervals, 'buy', smart_intervals_value, true)
          return buy_params unless buy_params.success?

          place_order(buy_params.data)
        end

        def sell(base:, quote:, price:, force_smart_intervals:, smart_intervals_value:, is_legacy:)
          sell_params = get_params(base, quote, price, force_smart_intervals, 'sell', smart_intervals_value, is_legacy)
          return sell_params unless sell_params.success?

          place_order(sell_params.data)
        end

        private

        def get_params(base, quote, price, force_smart_intervals, side, smart_intervals_value, price_in_quote)
          symbol = @market.symbol(base, quote)
          common_params = common_order_params(symbol, price, force_smart_intervals, smart_intervals_value, price_in_quote)
          return common_params unless common_params.success?

          Result::Success.new(
            common_params.data.merge(
              side: side
            )
          )
        end

        def common_order_params(symbol, price, force_smart_intervals, smart_intervals_value, price_in_quote)
          rate = @market.current_bid_price(symbol).data
          price_above_minimums = price_in_quote ?
                                   transaction_price(symbol, price, force_smart_intervals, smart_intervals_value)
                                   :
                                   smart_volume(symbol, price, rate, force_smart_intervals, smart_intervals_value, price_in_quote)
          return price_above_minimums unless price_above_minimums.success?
          if price_in_quote
            Result::Success.new(
              super(symbol).merge(
              minor: price_above_minimums.data.to_s,
              type: 'market'
            )
            )

            else
              Result::Success.new(
                super(symbol).merge(
                major: price_above_minimums.data.to_s,
                type: 'market'
              )
              )
            end
        end
      end
    end
  end
end
