module ExchangeApi
  module Clients
    class Deribit < ExchangeApi::Clients::Base
      def validate_credentials
        false
      end
    end
  end
end
