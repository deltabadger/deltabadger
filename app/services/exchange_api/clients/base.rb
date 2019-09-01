module ExchangeApi
  module Clients
    class Base
      def validate_credentials
        raise NotImplementedError
      end
    end
  end
end
