module ExchangeApi
  module Validators
    module Ftx
      class Validator < BaseValidator
        include ExchangeApi::Clients::Ftx

        URL = API_URL + '/api/account'.freeze

        def validate_credentials(api_key:, api_secret:)
          request = Faraday.get(URL, nil, headers(api_key, api_secret, nil, '/api/account'))

          return false if request.status != 200

          request.reason_phrase == 'OK'
        rescue StandardError
          false
        end
      end
    end
  end
end
