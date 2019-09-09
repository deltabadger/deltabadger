module ExchangeApi
  module Clients
    class Bitbay < ExchangeApi::Clients::Base
      def validate_credentials
        false
      end

      def buy
        puts 'Buying on bitbay'
      end
    end
  end
end
