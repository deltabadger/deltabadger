module ExchangeApi
  module Validators
    module Bitstamp
      class WithdrawalValidator < BaseValidator
        include ExchangeApi::Clients::Bitstamp
        ORDER_NOT_FOUND_MESSAGE = 'Order not found.'.freeze

        def validate_credentials(api_key:, api_secret:)
          byebug
          path = '/api/v2/withdrawal-requests/'
          url = "#{API_URL}#{path}"
          body = { timedelta: 86400 }
          request = Faraday.post(url, body.to_query, headers(api_key, api_secret, body, path, 'POST'))

          request.reason_phrase != 'Authentication Failed'
        rescue StandardError
          false
        end
      end
    end
  end
end
