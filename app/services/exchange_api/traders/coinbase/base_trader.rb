require 'result'

# rubocop#disable Style/StringLiterals
module ExchangeApi
  module Traders
    module Coinbase
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Coinbase

        def initialize(
          api_key:,
          api_secret:,
          passphrase:,
          market: ExchangeApi::Markets::Coinbase::Market.new,
          map_errors: ExchangeApi::MapErrors::Coinbase.new
        )
          @api_key = api_key
          @api_secret = api_secret
          @market = market
          @map_errors = map_errors
        end

        API_URL = 'https://api.pro.coinbase.com'.freeze

        private

        def place_order(order_params)
          path = '/orders'.freeze
          url = API_URL + path
          body = params.to_json
          request = Faraday.post(url, body, headers(@api_key, @api_secret, @passphrase, body, path, 'POST'))
          parse_request(request)
        rescue StandardError
          Result::Failure.new('Could not make Coinbase order', RECOVERABLE)
        end

        def transaction_price(symbol, price, force_smart_intervals)
          min_price = @market.minimum_order_price(symbol)
          return min_price unless min_price.success?

          return Result::Success.new(min_price.data) if force_smart_intervals

          Result::Success.new([min_price.data, price].max)
        end

        def transaction_volume(symbol, price, rate)
          rate_decimals = @market.base_decimals(symbol)
          return rate_decimals unless rate_decimals.success?

          Result::Success.new((price / rate).ceil(rate_decimals.data))
        end

        def common_order_params
          {}
        end

        def parse_request(request)
          if request.status == 200 && request.reason_phrase == 'OK'
            response = JSON.parse(request.body)
            # TODO: check if rate and amount are correct
            Result::Success.new(
              offer_id: response.fetch('id'),
              rate: response.fetch('price').to_f,
              amount: response.fetch('size').to_f
            )
          else
            error_to_failure(response.fetch('errors'))
          end
        end
      end
    end
  end
end
