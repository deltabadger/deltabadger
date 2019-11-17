module ExchangeApi
  module Clients
    class Fake
      SUCCESS = true
      # SUCCESS = false

      def initialize(exchange_name)
        @exchange_name = exchange_name
      end

      def validate_credentials
        true
      end

      def current_value
        100
      end

      def buy(_)
        puts "Fake: Buying things on #{@exchange_name}!"

        if SUCCESS
          Result::Success.new(
            offer_id: SecureRandom.uuid,
            rate: rand,
            amount: rand(3000)
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
            rate: rand,
            amount: rand(30)
          )
        else
          Result::Failure.new('Something went wrong!')
        end
      end
    end
  end
end
