module ExchangeApi
  module Validators
    module CoinbasePro
      class WithdrawalValidator < BaseValidator
        def validate_credentials(api_key:, api_secret:)
          true
        rescue StandardError
          false
        end
      end
    end
  end
end
