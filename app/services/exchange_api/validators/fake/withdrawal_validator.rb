module ExchangeApi
  module Validators
    module Fake
      class WithdrawalValidator < BaseValidator
        SUCCESS = true

        def validate_credentials(api_key:, api_secret:) # rubocop:disable Lint/UnusedMethodArgument
          SUCCESS
        end
      end
    end
  end
end
