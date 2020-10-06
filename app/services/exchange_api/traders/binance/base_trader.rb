require 'result'

module ExchangeApi
  module Traders
    module Binance
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Binance

        DEFAULT_MIN_QUANTITY = 0.001
        DEFAULT_QUANTITY_ACCURACY = 3 # Decimal places

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
          [min_price, price].max
        end

        def transaction_quantity(price, rate)
          quantity = (price / rate).round(DEFAULT_QUANTITY_ACCURACY)
          [quantity, DEFAULT_MIN_QUANTITY].max
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
