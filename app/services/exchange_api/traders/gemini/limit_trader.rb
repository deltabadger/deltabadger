module ExchangeApi
  module Traders
    module Gemini
      class LimitTrader < ExchangeApi::Traders::Gemini::BaseTrader
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

          limit_rate = rate_percentage(symbol, rate.data, -percentage)
          return limit_rate unless limit_rate.success?

          volume = smart_volume(symbol, price, limit_rate.data, force_smart_intervals)
          return volume unless volume.success?

          Result::Success
            .new(common_order_params(symbol).merge(
                   side: 'buy',
                   amount: volume.data,
                   price: limit_rate.data
                 ))
        end

        def get_sell_params(symbol, price, percentage, force_smart_intervals)
          rate = @market.current_bid_price(symbol)
          return rate unless rate.success?

          limit_rate = rate_percentage(symbol, rate.data, percentage)
          return limit_rate unless limit_rate.success?

          volume = smart_volume(symbol, price, limit_rate.data, force_smart_intervals)
          return volume unless volume.success?

          Result::Success
            .new(common_order_params(symbol)
                   .merge(
                     side: 'sell',
                     amount: volume.data,
                     price: limit_rate.data
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
            response.data.merge(rate: order_params[:price], amount: order_params[:amount])
          )
        rescue StandardError
          Result::Failure.new('Could not make Gemini order', RECOVERABLE)
        end

        def parse_request(request)
          response = JSON.parse(request.body)
          if request.status == 200 && request.reason_phrase == 'OK'
            order_id = response.fetch('id')

            Result::Success.new(offer_id: order_id)
          else
            error_to_failure([response.fetch('reason')])
          end
        end

        def common_order_params(symbol)
          super.merge(type: 'exchange limit')
        end
      end
    end
  end
end

