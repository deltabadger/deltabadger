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
          @passphrase = passphrase
          @market = market
          @map_errors = map_errors
        end

        API_URL = 'https://api.pro.coinbase.com'.freeze

        private

        def place_order(order_params)
          path = '/orders'.freeze
          url = API_URL + path
          body = order_params.to_json
          request = Faraday.post(url, body, headers(@api_key, @api_secret, @passphrase, body, path, 'POST'))
          parse_request(request)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not make Coinbase order', RECOVERABLE)
        end

        def transaction_price(symbol, price, force_smart_intervals)
          min_price = @market.minimum_order_price(symbol)
          return min_price unless min_price.success?

          return Result::Success.new(min_price.data) if force_smart_intervals

          Result::Success.new([min_price.data, price].max)
        end

        def common_order_params(symbol)
          {
            product_id: symbol
          }
        end

        def parse_request(request)
          response = JSON.parse(request.body)
          if request.status == 200 && request.reason_phrase == 'OK'
            response = JSON.parse(request.body)
            # TODO: check if rate and amount are correct
            Result::Success.new(
              offer_id: response.fetch('id'),
              #rate: response.fetch('price').to_f,
              amount: response.fetch('size').to_f
            )
          else
            error_to_failure([response.fetch('message')])
          end
        end
      end
    end
  end
end
