module ExchangeApi
  module Validators
    module Probit
      class Validator < BaseValidator
        include ExchangeApi::Clients::Probit
        def validate_credentials(api_key:, api_secret:)
          response_status = get_token(api_key, api_secret)[:status]
          return false if response_status != 200

          true
        rescue StandardError
          false
        end
      end
    end
  end
end
