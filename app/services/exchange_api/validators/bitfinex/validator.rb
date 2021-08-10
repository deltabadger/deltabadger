module ExchangeApi
  module Validators
    module Bitfinex
      class Validator < BaseValidator
        include ExchangeApi::Clients::Bitfinex
        FAKE_ORDER_ID = '9' * 10
        ORDER_NOT_FOUND_MESSAGE = 'id: invalid'.freeze

        def validate_credentials(api_key:, api_secret:)
          path = '/auth/w/order/cancel'
          url = "#{PRIVATE_API_URL}#{path}"
          body = { id: FAKE_ORDER_ID }

          request = Faraday.post(url, body.to_json, headers(api_key, api_secret, body, path))
          response = JSON.parse(request.body)

          response[2] == ORDER_NOT_FOUND_MESSAGE
        rescue StandardError
          false
        end
      end
    end
  end
end
