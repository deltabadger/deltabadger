module ExchangeApi
  module Validators
    module Bybit
      class Validator < BaseValidator
        def validate_credentials(api_key:, api_secret:)
          result = Clients::Bybit.new(
            api_key: api_key,
            api_secret: api_secret
          ).wallet_balance(account_type: 'UNIFIED')

          return false if result.failure?

          result.data['retCode'].zero?
        rescue StandardError
          false
        end
      end
    end
  end
end
