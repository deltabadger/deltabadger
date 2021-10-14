module ExchangeApi
  module Validators
    module Bitbay
      class WithdrawalValidator < BaseValidator
        include ExchangeApi::Clients::Bitbay
        URL = 'https://api.bitbay.net/rest/balances/BITBAY/balance'.freeze

        def validate_credentials(api_key:, api_secret:)
          request = Faraday.get(URL, {}, headers(api_key, api_secret, ''))
          return false if request.status != 200

          response = JSON.parse(request.body)
          response['status'] == 'Ok'
        rescue StandardError
          false
        end
      end
    end
  end
end
