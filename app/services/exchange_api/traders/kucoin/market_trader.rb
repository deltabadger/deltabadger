module ExchangeApi
  module Traders
    module Kucoin
      class MarketTrader < ExchangeApi::Traders::Kucoin::BaseTrader
        def buy(base:, quote:, price:, force_smart_intervals:, smart_intervals_value:)
          symbol = @market.symbol(base, quote)
          buy_params = get_buy_params(symbol, price, force_smart_intervals, smart_intervals_value)
          return buy_params unless buy_params.success?

          place_order(buy_params.data)
        end

        def sell(base:, quote:, price:, force_smart_intervals:, smart_intervals_value:, is_legacy:)
          symbol = @market.symbol(base, quote)
          sell_params = get_sell_params(symbol, price, force_smart_intervals, smart_intervals_value, is_legacy)
          return sell_params unless sell_params.success?

          place_order(sell_params.data)
        end

        private

        def get_buy_params(symbol, price, force_smart_intervals, smart_intervals_value)
          price_above_minimums = transaction_price(symbol, price, force_smart_intervals, smart_intervals_value, true)
          return price_above_minimums unless price_above_minimums.success?

          Result::Success.new(
            common_order_params(symbol).merge(
              funds: ConvertScientificToDecimal.new.call(price_above_minimums.data.to_f),
              side: 'buy'
            )
          )
        end

        def get_sell_params(symbol, price, force_smart_intervals, smart_intervals_value, price_in_quote)
          price_above_minimums = transaction_price(symbol, price, force_smart_intervals, smart_intervals_value, price_in_quote)
          return price_above_minimums unless price_above_minimums.success?

          if price_in_quote
            Result::Success.new(
              common_order_params(symbol).merge(
                funds: ConvertScientificToDecimal.new.call(price_above_minimums.data.to_f),
                side: 'sell'
              )
            )
          else
            Result::Success.new(
              common_order_params(symbol).merge(
                size: ConvertScientificToDecimal.new.call(price_above_minimums.data.to_f),
                side: 'sell'
              )
            )
          end
        end

        def common_order_params(symbol)
          super.merge(type: 'market')
        end
      end
    end
  end
end
