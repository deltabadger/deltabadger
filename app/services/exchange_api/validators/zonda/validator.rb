module ExchangeApi
  module Validators
    module Zonda
      class Validator < BaseValidator
        include ExchangeApi::Clients::Zonda

        URL = 'https://api.zondacrypto.exchange/rest/trading/history/transactions'.freeze

        def validate_credentials(api_key:, api_secret:)
          request = Faraday.get(URL, {}, headers(api_key, api_secret, '')) do |conn|
            conn.proxy = ENV['US_HTTPS_PROXY'].present? ? "https://#{ENV['US_HTTPS_PROXY']}" : nil
          end
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
