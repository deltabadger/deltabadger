module ExchangeApi
  module Clients
    class Base
      def validate_credentials
        raise NotImplementedError
      end

      def buy
        raise NotImplementedError
      end

      def sell
        raise NotImplementedError
      end
    end
  end
end
