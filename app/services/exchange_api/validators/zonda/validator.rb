module ExchangeApi
  module Validators
    module Zonda
      class Validator < BaseValidator
        include ExchangeApi::Clients::Zonda

        URL = 'https://api.zonda.exchange/rest/trading/history/transactions'.freeze

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
