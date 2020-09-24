require 'result'

# rubocop#disable Style/StringLiterals
module ExchangeApi
  module Clients
    module Bitbay
      class Base < ExchangeApi::Clients::Base
        MIN_TRANSACTION_PRICE = 10

        def initialize(api_key:, api_secret:, map_errors: ExchangeApi::MapErrors::Bitbay.new)
          @api_key = api_key
          @api_secret = api_secret
          @map_errors = map_errors
        end

        def current_bid_ask_price(currency)
          url =
            "https://bitbay.net/API/Public/BTC#{currency}/ticker.json"
          response = JSON.parse(Faraday.get(url, {}, headers('')).body)

          bid = response.fetch('bid').to_f
          ask = response.fetch('ask').to_f

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new('Could not fetch current price from Bitbay', RECOVERABLE)
        end

        private

        def place_order(currency, body)
          url = "https://api.bitbay.net/rest/trading/offer/BTC-#{currency}"
          response = JSON.parse(Faraday.post(url, body, headers(body)).body)
          parse_response(response)
        rescue StandardError
          Result::Failure.new('Could not make Bitbay order', RECOVERABLE)
        end

        def common_order_params(price)
          price = [MIN_TRANSACTION_PRICE, price].max
          {
            amount: nil,
            postOnly: false,
            fillOrKill: false,
            price: price
          }
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

        def headers(body)
          timestamp = Time.now.to_i.to_s
          post = @api_key + timestamp.to_s + body.to_s
          signature = OpenSSL::HMAC.hexdigest('sha512', @api_secret, post)

          {
            'API-Key' => @api_key,
            'API-Hash' => signature,
            'operation-id' => SecureRandom.uuid.to_s,
            'Request-Timestamp' => timestamp,
            'Content-Type' => 'application/json'
          }
        end
      end
    end
  end
end
