module ExchangeApi
  module Clients
    module Fake
      class Validator < BaseValidatr
        SUCCESS = true
        # SUCCESS = false

        def validate_credentials(_api_key, _api_secret)
          SUCCESS
        end
      end
    end
  end
end
