module ExchangeApi
  module Traders
    module Bitfinex
      class LimitTrader < ExchangeApi::Traders::Bitfinex::BaseTrader
        def buy(base:, quote:, price:, percentage:, force_smart_intervals:, smart_intervals_value:)
          symbol = @market.symbol(base, quote)
          buy_params = get_buy_params(symbol, price, percentage, force_smart_intervals, smart_intervals_value)
          return buy_params unless buy_params.success?

          place_order(buy_params.data)
        end

        def sell(base:, quote:, price:, percentage:, force_smart_intervals:, smart_intervals_value:, is_legacy:)
          symbol = @market.symbol(base, quote)
          sell_params = get_sell_params(symbol, price, percentage, force_smart_intervals, smart_intervals_value, is_legacy)
          return sell_params unless sell_params.success?

          place_order(sell_params.data)
        end

        def fetch_order_by_id(_order_id, response_params = nil)
          Result::Success.new(response_params)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch order parameters from Bitfinex')
        end

        private

        def get_buy_params(symbol, price, percentage, force_smart_intervals, smart_intervals_value)
          rate = @market.current_ask_price(symbol)
          return rate unless rate.success?

          limit_percentage = limit_buy_percentage(percentage)
          limit_rate = rate_percentage(rate.data, limit_percentage)
          return limit_rate unless limit_rate.success?

          volume = smart_volume(symbol, price, limit_rate.data, force_smart_intervals, smart_intervals_value, true)
          return volume unless volume.success?

          Result::Success
            .new(common_order_params(symbol).merge(
                   amount: volume.data.to_d,
                   price: limit_rate.data
                 ))
        end

        def get_sell_params(symbol, price, percentage, force_smart_intervals, smart_intervals_value, price_in_quote)
          rate = @market.current_bid_price(symbol)
          return rate unless rate.success?

          limit_percentage = limit_sell_percentage(percentage)
          limit_rate = rate_percentage(rate.data, limit_percentage)
          return limit_rate unless limit_rate.success?

          volume = smart_volume(symbol, price, limit_rate.data, force_smart_intervals, smart_intervals_value, price_in_quote)
          return volume unless volume.success?

          Result::Success
            .new(common_order_params(symbol)
                   .merge(
                     amount: -volume.data.to_d,
                     price: limit_rate.data
                   ))
        end

        def rate_percentage(rate, percentage)
          Result::Success.new((rate * (1 + percentage / 100)))
        end

        def place_order(order_params)
          response = super
          return response unless response.success?

          Result::Success.new(
            response.data.merge(rate: order_params[:price], amount: order_params[:amount].abs)
          )
        rescue StandardError
          Result::Failure.new('Could not make Bitfinex order', RECOVERABLE)
        end

        def common_order_params(symbol)
          super.merge(type: 'EXCHANGE LIMIT')
        end

        def limit_buy_percentage(percentage)
          -percentage
        end

        def limit_sell_percentage(percentage)
          percentage
        end
      end
    end
  end
end
