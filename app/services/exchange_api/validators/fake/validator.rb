module ExchangeApi
  module Validators
    module Fake
      class Validator < BaseValidator
        include ExchangeApi::Clients::Fake

        SUCCESS = true
        # SUCCESS = false

        def validate_credentials(_api_key, _api_secret)
          SUCCESS
        end
      end
    end
  end
end
