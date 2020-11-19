require 'result'

module ExchangeApi
  module Traders
    module Binance
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Binance

        def initialize(
          api_key:,
          api_secret:,
          market: ExchangeApi::Markets::Binance::Market.new,
          map_errors: ExchangeApi::MapErrors::Binance.new
        )
          @signed_client = signed_client(api_key, api_secret)
          @market = market
          @map_errors = map_errors
        end

        private

        def place_order(order_params)
          # Remove exponential notation from quantity
          if order_params.key?(:quantity)
            parsed_quantity = parse_quantity(order_params[:symbol], order_params[:quantity])
            return parsed_quantity unless parsed_quantity.success?

            order_params[:quantity] = parsed_quantity.data
          end

          request = @signed_client.post('order') do |req|
            req.params = order_params
          end

          response = JSON.parse(request.body)

          parse_response(response)
        rescue StandardError
          Result::Failure.new('Could not make Binance order', RECOVERABLE)
        end

        def common_order_params(symbol)
          {
            symbol: symbol
          }
        end

        def transaction_price(symbol, price, force_smart_intervals)
          min_price = @market.minimum_order_price(symbol)
          return min_price unless min_price.success?

          return min_price if force_smart_intervals

          Result::Success.new([price, min_price.data].max)
        end

        def transaction_volume(symbol, price, rate)
          min_volume = @market.minimum_order_volume(symbol)
          return min_volume unless min_volume.success?

          volume = chosen_volume(symbol, price, rate)
          return volume unless volume.success?

          Result::Success.new([volume.data, min_volume.data].max)
        end

        def chosen_volume(symbol, price, rate)
          base_step_size = @market.base_step_size(symbol)
          return base_step_size unless base_step_size.success?

          volume = price / rate
          Result::Success.new((volume / base_step_size.data).ceil * base_step_size.data)
        end

        def parse_response(response)
          return error_to_failure([response['msg']]) if response['msg'].present?

          rate = BigDecimal(response['cummulativeQuoteQty']) / BigDecimal(response['executedQty'])
          Result::Success.new(
            offer_id: response['orderId'],
            rate: rate,
            amount: response['executedQty']
          )
        end

        def parse_quantity(symbol, quantity)
          base_decimals = @market.base_decimals(symbol)
          return base_decimals unless base_decimals.success?

          Result::Success.new("%.#{base_decimals.data}f" % quantity)
        end
      end
    end
  end
end
