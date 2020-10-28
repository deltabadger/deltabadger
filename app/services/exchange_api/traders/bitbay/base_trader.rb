require 'result'

# rubocop#disable Style/StringLiterals
module ExchangeApi
  module Traders
    module Bitbay
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Bitbay

        def initialize(
          api_key:,
          api_secret:,
          market: ExchangeApi::Markets::Bitbay::Market.new,
          map_errors: ExchangeApi::MapErrors::Bitbay.new
        )
          @api_key = api_key
          @api_secret = api_secret
          @market = market
          @map_errors = map_errors
        end

        private

        def place_order(symbol, params)
          url = "https://api.bitbay.net/rest/trading/offer/#{symbol}"
          body = params.to_json
          response = JSON.parse(Faraday.post(url, body, headers(@api_key, @api_secret, body)).body)
          parse_response(response)
        rescue StandardError
          Result::Failure.new('Could not make Bitbay order', RECOVERABLE)
        end

        def transaction_price(symbol, price)
          min_price = @market.minimum_order_price(symbol)
          return min_price unless min_price.success?

          Result::Success.new([min_price.data, price].max)
        end

        def transaction_volume(symbol, price, rate)
          rate_decimals = @market.base_decimals(symbol)
          return rate_decimals unless rate_decimals.success?

          Result::Success.new((price / rate).ceil(rate_decimals.data))
        end

        def common_order_params
          { postOnly: false, fillOrKill: false }
        end

        def parse_response(response)
          if response.fetch('status') == 'Ok'
            Result::Success.new(
              offer_id: response.fetch('offerId'),
              rate: response.fetch('transactions').first.fetch('rate'),
              amount: response.fetch('transactions').first.fetch('amount')
            )
          else
            error_to_failure(response.fetch('errors'))
          end
        end
      end
    end
  end
end
