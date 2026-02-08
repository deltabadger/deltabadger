module ExchangeApi
  module Validators
    module Bitget
      class Validator < BaseValidator
        API_URL = 'https://api.bitget.com'.freeze

        def validate_credentials(api_key:, api_secret:, passphrase: nil)
          client = Clients::Bitget.new(
            api_key: api_key,
            api_secret: api_secret,
            passphrase: passphrase
          )
          result = client.get_assets
          result.success? && result.data['code'] == '00000'
        rescue StandardError
          false
        end
      end
    end
  end
end
