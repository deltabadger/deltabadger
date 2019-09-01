module ExchangeApi
  module Clients
    class Deribit < ExchangeApi::Clients::Base
      def validate_credentials
        false
      end

      def buy
        puts 'Buying on derbit'
      end
    end
  end
end
