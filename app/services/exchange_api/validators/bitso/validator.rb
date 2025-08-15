module ExchangeApi
  module Validators
    module Bitso
      class Validator < BaseValidator
        include ExchangeApi::Clients::Bitso

        URL = API_URL + '/v3/account_status/'.freeze

        def validate_credentials(api_key:, api_secret:)
          conn = Faraday.new(proxy: ENV['US_HTTPS_PROXY'])
          request = conn.get(URL, nil, headers(api_key, api_secret, nil, '/v3/account_status/'))

          return false if request.status != 200

          request.reason_phrase == 'OK'
        rescue StandardError
          false
        end
      end
    end
  end
end
