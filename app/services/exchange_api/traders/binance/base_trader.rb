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

        def transaction_price(symbol, price)
          min_price = @market.minimum_order_price(symbol)
          return min_price unless min_price.success?

          [price, min_price.data].max
        end

        def transaction_volume(price, rate)
          min_volume = @market.minimum_order_volume(symbol)
          return min_volume unless min_volume.success?

          [chosen_volume(price, rate), min_volume.data].max
        end

        def chosen_volume(price, rate)
          base_step_size = @market.base_step_size(symbol)
          return base_step_size unless base_step_size.success?

          base_decimals = @market.base_decimals(symbol)
          return base_decimals unless base_step_size.success?

          volume = price / rate
          ((volume / base_step_size.data).round * base_step_size).round(base_decimals.data)
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
      end
    end
  end
end
