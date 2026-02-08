module ExchangeApi
  module Validators
    module Gemini
      class Validator < BaseValidator
        API_URL = 'https://api.gemini.com'.freeze

        def validate_credentials(api_key:, api_secret:)
          client = Clients::Gemini.new(
            api_key: api_key,
            api_secret: api_secret
          )
          result = client.get_balances
          result.success? && result.data.is_a?(Array)
        rescue StandardError
          false
        end
      end
    end
  end
end
