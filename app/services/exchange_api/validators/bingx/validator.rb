module ExchangeApi
  module Validators
    module Bingx
      class Validator < BaseValidator
        def validate_credentials(api_key:, api_secret:)
          result = Clients::Bingx.new(
            api_key: api_key,
            api_secret: api_secret
          ).get_balances

          result.success? && result.data['code'].to_i.zero?
        rescue StandardError
          false
        end
      end
    end
  end
end
