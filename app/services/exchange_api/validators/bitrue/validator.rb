module ExchangeApi
  module Validators
    module Bitrue
      class Validator < BaseValidator
        def validate_credentials(api_key:, api_secret:)
          result = Clients::Bitrue.new(
            api_key: api_key,
            api_secret: api_secret
          ).account_information

          result.success?
        rescue StandardError
          false
        end
      end
    end
  end
end
