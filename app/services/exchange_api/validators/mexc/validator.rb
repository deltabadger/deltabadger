module ExchangeApi
  module Validators
    module Mexc
      class Validator < BaseValidator
        def validate_credentials(api_key:, api_secret:)
          result = Honeymaker.client('mexc',
                                     api_key: api_key,
                                     api_secret: api_secret).validate(:trading)

          result.success?
        rescue StandardError
          false
        end
      end
    end
  end
end
