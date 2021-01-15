module ExchangeApi
  module Traders
    module Coinbase
      class LimitTrader < ExchangeApi::Traders::Coinbase::BaseTrader
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


        def get_buy_params(symbol, price, percentage, force_smart_intervals)
          rate = @market.current_ask_price(symbol)
          return rate unless rate.success?

          limit_rate = rate_percentage(symbol, rate.data, percentage)
          return limit_rate unless limit_rate.success?

          price_above_minimums = transaction_price(symbol, price, force_smart_intervals)
          return price_above_minimums unless price_above_minimums.success?

          amount = transaction_volume(symbol, price_above_minimums.data, limit_rate.data)
          return amount unless amount.success?

          Result::Success.new(common_order_params.merge(
            offerType: 'buy',
            amount: amount.data,
            rate: limit_rate.data
          ))
        end

        def get_sell_params(symbol, price, percentage, force_smart_intervals)
          rate = @market.current_bid_price(symbol)
          return rate unless rate.success?

          limit_rate = rate_percentage(symbol, rate.data, percentage)
          return limit_rate unless limit_rate.success?

          price_above_minimums = transaction_price(symbol, price, force_smart_intervals)
          return price_above_minimums unless price_above_minimums.success?

          amount = transaction_volume(symbol, price_above_minimums.data, limit_rate.data)
          return amount unless amount.success?

          Result::Success.new(common_order_params.merge(
            offerType: 'sell',
            amount: amount.data,
            rate: limit_rate.data
          ))
        end

        def rate_percentage(symbol, rate, percentage)
          rate_decimals = @market.quote_decimals(symbol)
          return rate_decimals unless rate_decimals.success?

          Result::Success.new((rate * (1 + percentage / 100)).ceil(rate_decimals.data))
        end

        def common_order_params
          super.merge(mode: 'limit')
        end
      end
    end
  end
end

