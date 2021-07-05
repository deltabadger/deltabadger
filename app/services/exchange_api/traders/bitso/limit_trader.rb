module ExchangeApi
  module Traders
    module Bitso
      class LimitTrader < ExchangeApi::Traders::Bitso::BaseTrader
        def buy(base:, quote:, price:, percentage:, force_smart_intervals:, smart_intervals_value:)
          symbol = @market.symbol(base, quote)
          buy_params = get_buy_params(symbol, price, percentage, force_smart_intervals, smart_intervals_value)
          return buy_params unless buy_params.success?

          place_order(buy_params.data)
        end

        def sell(base:, quote:, price:, percentage:, force_smart_intervals:, smart_intervals_value:)
          symbol = @market.symbol(base, quote)
          sell_params = get_sell_params(symbol, price, percentage, force_smart_intervals, smart_intervals_value)
          return sell_params unless sell_params.success?

          place_order(sell_params.data)
        end

        def fetch_order_by_id(order_id, response_params = nil)
          Result::Success.new(response_params)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch order parameters from Bitso')
        end

        private

        def get_buy_params(symbol, price, percentage, force_smart_intervals, smart_intervals_value)
          rate = @market.current_ask_price(symbol)
          return rate unless rate.success?

          limit_rate = rate_percentage(symbol, rate.data, -percentage)
          return limit_rate unless limit_rate.success?

          volume = smart_volume(symbol, price, limit_rate.data, force_smart_intervals, smart_intervals_value)
          return volume unless volume.success?

          Result::Success
            .new(common_order_params(symbol).merge(
                   side: 'buy',
                   major: volume.data.to_s,
                   price: limit_rate.data.to_s
                 ))
        end

        def get_sell_params(symbol, price, percentage, force_smart_intervals, smart_intervals_value)
          rate = @market.current_bid_price(symbol)
          return rate unless rate.success?

          limit_rate = rate_percentage(symbol, rate.data, percentage)
          return limit_rate unless limit_rate.success?

          volume = smart_volume(symbol, price, limit_rate.data, force_smart_intervals, smart_intervals_value)
          return volume unless volume.success?

          Result::Success
            .new(common_order_params(symbol)
                   .merge(
                     side: 'sell',
                     major: volume.data.to_s,
                     price: limit_rate.data.to_s
                   ))
        end

        def rate_percentage(symbol, rate, percentage)
          rate_decimals = @market.quote_decimals(symbol)
          return rate_decimals unless rate_decimals.success?

          Result::Success.new((rate * (1 + percentage / 100)).ceil(rate_decimals.data))
        end

        def place_order(order_params)
          response = super
          return response unless response.success?

          Result::Success.new(
            response.data.merge(rate: order_params[:price], amount: order_params[:major])
          )
        rescue StandardError
          Result::Failure.new('Could not make Bitso order', RECOVERABLE)
        end

        def parse_request(request)
          response = JSON.parse(request.body)
          if request.status == 200 && request.reason_phrase == 'OK'
            order_id = response.fetch('payload').fetch('oid')

            Result::Success.new(offer_id: order_id)
          else
            error_to_failure([response.fetch('error').fetch('code')])
          end
        end

        def common_order_params(symbol)
          super.merge(type: 'limit')
        end
      end
    end
  end
end

