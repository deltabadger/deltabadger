require 'result'

# rubocop#disable Style/StringLiterals
module ExchangeApi
  module Traders
    module Bitbay
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Bitbay

        MIN_TRANSACTION_PRICE = 10

        def initialize(api_key:, api_secret:, map_errors: ExchangeApi::MapErrors::Bitbay.new)
          @api_key = api_key
          @api_secret = api_secret
          @map_errors = map_errors
        end

        def current_bid_ask_price(currency)
          url =
            "https://bitbay.net/API/Public/BTC#{currency}/ticker.json"
          response = JSON.parse(Faraday.get(url, {}, headers(@api_key, @api_secret, '')).body)

          bid = response.fetch('bid').to_f
          ask = response.fetch('ask').to_f

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new('Could not fetch current price from Bitbay', RECOVERABLE)
        end

        private

        def place_order(currency, params)
          url = "https://api.bitbay.net/rest/trading/offer/BTC-#{currency}"
          body = params.to_json
          response = JSON.parse(Faraday.post(url, body, headers(@api_key, @api_secret, body)).body)
          parse_response(response)
        rescue StandardError
          Result::Failure.new('Could not make Bitbay order', RECOVERABLE)
        end

        def transaction_price(price)
          [MIN_TRANSACTION_PRICE, price].max
        end

        def transaction_amount(price, rate)
          (price / rate).ceil(8)
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
