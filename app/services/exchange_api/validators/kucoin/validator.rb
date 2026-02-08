module ExchangeApi
  module Validators
    module Kucoin
      class Validator < BaseValidator
        API_URL = 'https://api.kucoin.com'.freeze

        def validate_credentials(api_key:, api_secret:, passphrase: nil)
          client = Clients::Kucoin.new(
            api_key: api_key,
            api_secret: api_secret,
            passphrase: passphrase
          )
          result = client.get_accounts(type: 'trade')
          result.success? && result.data['code'] == '200000'
        rescue StandardError
          false
        end
      end
    end
  end
end
