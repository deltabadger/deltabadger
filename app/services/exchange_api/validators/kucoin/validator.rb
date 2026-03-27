module ExchangeApi
  module Validators
    module Kucoin
      class Validator < BaseValidator
        API_URL = 'https://api.kucoin.com'.freeze

        def validate_credentials(api_key:, api_secret:, passphrase: nil)
          result = Honeymaker.client('kucoin', api_key: api_key, api_secret: api_secret, passphrase: passphrase).validate(:trading)
          result.success?
        rescue StandardError
          false
        end
      end
    end
  end
end
