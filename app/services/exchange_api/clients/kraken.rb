require 'kraken_ruby_client'

module ExchangeApi
  module Clients
    class Kraken < ExchangeApi::Clients::Base
      def initialize(api_key:, api_secret:, map_errors: ExchangeApi::MapErrors::Kraken.new)
        @client =
          ::Kraken::Client.new(api_key: api_key, api_secret: api_secret)
        @map_errors = map_errors
      end

      def validate_credentials
        response = @client.balance

        return false if response.keys.include?('error')

        true
      end

      def current_price(settings)
        currency = settings.fetch('currency')
        result = @client.ticker("xbt#{currency}")['result']["XXBTZ#{currency}"]

        bid = result.fetch('b').first.to_f
        ask = result.fetch('a').first.to_f

        (bid + ask) / 2
      end

      def buy(settings)
        currency = settings.fetch('currency')
        price = settings.fetch('price')
        pair = "XBT#{currency}"

        response = @client.add_order(pair: pair, type: 'buy', ordertype: 'market', volume: 1)

        if response.fetch("error").any?
          Result::Failure.new(
            *@map_errors.call(response.fetch('error'))
          )
        else
          true
        end
      end

      def sell(_)
        puts 'Selling things on Kraken!'

        true
      end
    end
  end
end
