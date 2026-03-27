module ExchangeApi
  module Validators
    module Gemini
      class Validator < BaseValidator
        API_URL = 'https://api.gemini.com'.freeze

        def validate_credentials(api_key:, api_secret:)
          result = Honeymaker.client('gemini', api_key: api_key, api_secret: api_secret).validate(:trading)
          result.success?
        rescue StandardError
          false
        end
      end
    end
  end
end
