module ExchangeApi
  module Clients
    class Fake
      def initialize(exchange_name)
        @exchange_name = exchange_name
      end

      def validate_credentials
        true
      end

      def buy(_)
        puts "Fake: Buying things on #{@exchange_name}!"

        true
      end

      def sell(_)
        puts "Fake: Selling things on #{@exchange_name}!"

        true
      end
    end
  end
end
