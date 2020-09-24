module ExchangeApi
  module Clients
    class BaseValidator < BaseClient
      def validate_credentials(_api_key, _api_secret)
        raise NotImplementedError
      end
    end
  end
end
