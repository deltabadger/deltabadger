module ExchangeApi
  module Traders
    module CoinbasePro
      class MarketTrader < ExchangeApi::Traders::CoinbasePro::BaseTrader
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
          limit_only = @market.limit_only?(symbol)
          return limit_only unless limit_only.success?

          if limit_only.data
            return get_limit_only_params(symbol, price, force_smart_intervals, 'buy', smart_intervals_value)
          end

          price_above_minimums = transaction_price(symbol, price, force_smart_intervals, smart_intervals_value, true)
          return price_above_minimums unless price_above_minimums.success?

          Result::Success.new(
            common_order_params(symbol).merge(
              funds: price_above_minimums.data.to_f,
              side: 'buy'
            )
          )
        end

        def get_sell_params(symbol, price, force_smart_intervals, smart_intervals_value, price_in_quote)
          price_in_quote ||= force_smart_intervals
          limit_only = @market.limit_only?(symbol)
          return limit_only unless limit_only.success?

          if limit_only.data
            return get_limit_only_params(symbol, price, force_smart_intervals, 'sell', smart_intervals_value)
          end

          price_above_minimums = transaction_price(symbol, price, force_smart_intervals, smart_intervals_value, price_in_quote)
          return price_above_minimums unless price_above_minimums.success?

          if price_in_quote
            Result::Success.new(
              common_order_params(symbol).merge(
                funds: price_above_minimums.data.to_f,
                side: 'sell'
              )
            )
          else
            Result::Success.new(
              common_order_params(symbol).merge(
                size: price_above_minimums.data.to_f,
                side: 'sell'
              )
            )
          end
        end

        def get_limit_only_params(symbol, price, force_smart_intervals, side, smart_intervals_value)
          rate = @market.current_bid_price(symbol)
          return rate unless rate.success?

          percentage = side == 'buy' ? 0.5 : -0.5

          limit_rate = rate_percentage(symbol, rate.data, percentage)
          return limit_rate unless limit_rate.success?

          for_sell = side == 'sell'
          volume = smart_volume(symbol, price, limit_rate.data, force_smart_intervals, smart_intervals_value, for_sell)
          return volume unless volume.success?

          Result::Success
            .new(common_order_params(symbol, true)
              .merge(
                side: side,
                size: volume.data,
                price: limit_rate.data
              )
            )
        end

        def rate_percentage(symbol, rate, percentage)
          rate_decimals = @market.quote_decimals(symbol)
          return rate_decimals unless rate_decimals.success?

          Result::Success.new((rate * (1 + percentage / 100)).ceil(rate_decimals.data))
        end

        def common_order_params(symbol, limit_only = false)
          super.merge(type: limit_only ? 'limit' : 'market')
        end
      end
    end
  end
end
