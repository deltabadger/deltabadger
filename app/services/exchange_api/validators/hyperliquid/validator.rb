module ExchangeApi
  module Validators
    module Hyperliquid
      class Validator < BaseValidator
        def validate_credentials(api_key:, api_secret:)
          result = Honeymaker.client('hyperliquid',
                                     api_key: api_key,
                                     api_secret: api_secret).validate(:trading)

          result.success?
        rescue StandardError
          true
        end
      end
    end
  end
end
