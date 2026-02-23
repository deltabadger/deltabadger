module ExchangeApi
  module Validators
    module BitMart
      class Validator < BaseValidator
        def validate_credentials(api_key:, api_secret:, passphrase: nil)
          result = Clients::BitMart.new(
            api_key: api_key,
            api_secret: api_secret,
            memo: passphrase
          ).get_wallet

          result.success? && result.data['code'] == 1000
        rescue StandardError
          false
        end
      end
    end
  end
end
