module ExchangeApi
  module Validators
    module CoinbasePro
      class Validator < BaseValidator
        include ExchangeApi::Clients::CoinbasePro

        URL = 'https://api.pro.coinbase.com/fees'.freeze

        def validate_credentials(api_key:, api_secret:, passphrase:)
          request = Faraday.get(URL, {}, headers(api_key, api_secret, passphrase, '', '/fees')) do |conn|
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
