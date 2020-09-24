module ExchangeApi
  module Clients
    module Bitbay
      class Validator < BaseValidator
        URL = 'https://api.bitbay.net/rest/trading/history/transactions'.freeze

        def validate_credentials(api_key, api_secret)
          request = Faraday.get(URL, {}, headers(api_key, api_secret))
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