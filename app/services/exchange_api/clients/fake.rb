module ExchangeApi
  module Clients
    class Fake
      SUCCESS = true

      def initialize(exchange_name)
        @exchange_name = exchange_name
      end

      def validate_credentials
        true
      end

      def buy(_)
        puts "Fake: Buying things on #{@exchange_name}!"

        if SUCCESS
          Result::Success.new(
            offer_id: SecureRandom.uuid,
            rate: 20.123,
            amount: 30.2
          )
        else
          Result::Failure.new('Something went wrong!')
        end
      end

      def sell(_)
        puts "Fake: Selling things on #{@exchange_name}!"

        if SUCCESS
          Result::Success.new(
            offer_id: SecureRandom.uuid,
            rate: 20.123,
            amount: 30.2
          )
        else
          Result::Failure.new('Something went wrong!')
        end
      end
    end
  end
end
