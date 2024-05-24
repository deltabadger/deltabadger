module ExchangeApi
  module Validators
    module Coinbase
      class Validator < BaseValidator
        include ExchangeApi::Clients::Coinbase

        API_URL = 'https://api.coinbase.com'.freeze

        def validate_credentials(api_key:, api_secret:)
          path = '/api/v3/brokerage/transaction_summary'.freeze
          url = API_URL + path
          request = Faraday.get(url, nil, headers(api_key, api_secret, '', url, 'GET'))
          return false if request.status != 200

          true
        rescue StandardError
          false
        end
      end
    end
  end
end