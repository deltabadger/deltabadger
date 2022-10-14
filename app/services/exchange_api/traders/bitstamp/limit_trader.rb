module ExchangeApi
  module Traders
    module Bitstamp
      class LimitTrader < ExchangeApi::Traders::Bitstamp::BaseTrader
        def buy(base:, quote:, price:, percentage:, force_smart_intervals:, smart_intervals_value:)
          symbol = @market.symbol(base, quote)
          buy_params = get_params(symbol, price, percentage, force_smart_intervals,
                                  smart_intervals_value, 'buy', true)
          return buy_params unless buy_params.success?

          place_order(buy_params.data, 'buy', symbol, 'limit')
        end

        def sell(base:, quote:, price:, percentage:, force_smart_intervals:, smart_intervals_value:, is_legacy:)
          symbol = @market.symbol(base, quote)
          sell_params = get_params(symbol, price, percentage, force_smart_intervals,
                                   smart_intervals_value, 'sell', is_legacy)
          return sell_params unless sell_params.success?

          place_order(sell_params.data, 'sell', symbol, 'limit')
        end

        private

        def get_params(symbol, price, percentage, force_smart_intervals, smart_intervals_value, side, price_in_quote)
          rate = get_rate(side, symbol)
          return rate unless rate.success?

          limit_percentage = get_limit_percentage(percentage, side)
          limit_rate = rate_percentage(symbol, rate.data, limit_percentage)
          return limit_rate unless limit_rate.success?

          volume = smart_volume(symbol, price, limit_rate.data, force_smart_intervals, smart_intervals_value, price_in_quote)
          return volume unless volume.success?

          Result::Success
            .new(amount: volume.data, price: limit_rate.data)
        end

        def rate_percentage(symbol, rate, percentage)
          rate_decimals = @market.quote_decimals(symbol)
          return rate_decimals unless rate_decimals.success?

          Result::Success.new((rate * (1 + percentage / 100)).ceil(rate_decimals.data))
        end

        def place_order(order_params, side, symbol, type)
          response = super
          return response unless response.success?

          Result::Success.new(
            response.data.merge(rate: order_params[:price], amount: order_params[:amount])
          )
        rescue StandardError
          Result::Failure.new('Could not make Bitstamp order', **RECOVERABLE)
        end

        def get_limit_percentage(percentage, side)
          side == 'buy' ? -percentage : percentage
        end

        def get_rate(side, symbol)
          side == 'buy' ? @market.current_ask_price(symbol) : @market.current_bid_price(symbol)
        end
      end
    end
  end
end
