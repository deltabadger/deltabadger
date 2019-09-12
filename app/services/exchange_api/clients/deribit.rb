module ExchangeApi
  module Clients
    class Deribit < ExchangeApi::Clients::Base
      def validate_credentials
        false
      end

      def buy(_)
        puts 'Buying on derbit'
      end

      def sell(_)
        puts 'Selling on derbit'
      end
    end
  end
end
