module ExchangeApi
  module Validators
    class Base
      def validate_credentials(_api_key, _api_secret)
        raise NotImplementedError
      end
    end
  end
end
