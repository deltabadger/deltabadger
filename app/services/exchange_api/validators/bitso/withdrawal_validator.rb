module ExchangeApi
  module Validators
    module Bitso
      class WithdrawalValidator < BaseValidator
        include ExchangeApi::Clients::Bitso

        URL = API_URL + '/v3/withdrawals/'.freeze

        def validate_credentials(api_key:, api_secret:)
          request = Faraday.get(URL, nil, headers(api_key, api_secret, nil, '/v3/withdrawals/'))

          return false if request.status != 200

          request.reason_phrase == 'OK'
        rescue StandardError
          false
        end
      end
    end
  end
end
