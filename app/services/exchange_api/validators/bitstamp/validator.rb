module ExchangeApi
  module Validators
    module Bitstamp
      class Validator < BaseValidator
        include ExchangeApi::Clients::Bitstamp
        FAKE_ORDER_ID = '9' * 10
        ORDER_NOT_FOUND_MESSAGE = 'Order not found.'.freeze

        def validate_credentials(api_key:, api_secret:)
          path = '/api/v2/order_status/'
          url = "#{API_URL}#{path}"
          body = { id: FAKE_ORDER_ID }
          conn = Faraday.new(proxy: ENV['US_HTTPS_PROXY'])
          request = conn.post(url, body.to_query, headers(api_key, api_secret, body, path, 'POST'))

          response = JSON.parse(request.body)
          response['reason'] == ORDER_NOT_FOUND_MESSAGE
        rescue StandardError
          false
        end
      end
    end
  end
end
