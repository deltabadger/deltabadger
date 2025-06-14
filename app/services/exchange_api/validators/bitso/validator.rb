module ExchangeApi
  module Validators
    module Bitso
      class Validator < BaseValidator
        include ExchangeApi::Clients::Bitso

        URL = API_URL + '/v3/account_status/'.freeze

        def validate_credentials(api_key:, api_secret:)
          request = Faraday.get(URL, nil, headers(api_key, api_secret, nil, '/v3/account_status/')) do |conn|
            conn.proxy = ENV['US_HTTPS_PROXY'].present? ? "https://#{ENV['US_HTTPS_PROXY']}" : nil
          end

          return false if request.status != 200

          request.reason_phrase == 'OK'
        rescue StandardError
          false
        end
      end
    end
  end
end
