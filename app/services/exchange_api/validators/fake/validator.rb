module ExchangeApi
  module Validators
    module Fake
      class Validator < BaseValidator
        include ExchangeApi::Clients::Fake

        SUCCESS = true
        # SUCCESS = false

        def validate_credentials(api_key:, api_secret:) # rubocop:disable Lint/UnusedMethodArgument
          SUCCESS
        end
      end
    end
  end
end
