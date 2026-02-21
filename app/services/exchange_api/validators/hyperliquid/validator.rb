module ExchangeApi
  module Validators
    module Hyperliquid
      class Validator < BaseValidator
        def validate_credentials(api_key:, api_secret:)
          client = Clients::Hyperliquid.new(
            wallet_address: api_key,
            agent_key: api_secret
          )
          # Try cancelling a non-existent order to test credentials
          client.cancel(coin: 'ETH', oid: 0)
          true
        rescue ::Hyperliquid::AuthenticationError
          false
        rescue StandardError
          true
        end
      end
    end
  end
end
