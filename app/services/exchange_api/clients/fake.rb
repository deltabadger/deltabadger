module ExchangeApi
  module Clients
    class Fake < ExchangeApi::Clients::Base
      SUCCESS = true
      # SUCCESS = false

      def initialize(exchange_name)
        @exchange_name = exchange_name
      end

      def validate_credentials
        SUCCESS
      end

      def current_price(_)
        100
      end

      def buy(_)
        puts "Fake: Buying things on #{@exchange_name}!"

        if SUCCESS
          Result::Success.new(
            offer_id: SecureRandom.uuid,
            rate: rand(6000...8000),
            amount: 0.002
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
