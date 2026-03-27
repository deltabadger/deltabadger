module ExchangeApi
  module Validators
    module Bitmart
      class Validator < BaseValidator
        def validate_credentials(api_key:, api_secret:, passphrase: nil)
          result = Honeymaker.client('bitmart',
                                     api_key: api_key,
                                     api_secret: api_secret,
                                     memo: passphrase).validate(:trading)

          result.success?
        rescue StandardError
          false
        end
      end
    end
  end
end
