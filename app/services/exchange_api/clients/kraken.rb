# rubocop:disable Metrics/LineLength:
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

        return false if response.fetch('error').any?

        true
      end

      def current_price(settings)
        currency = settings.fetch('currency')
        result = @client.ticker("xbt#{currency}")['result']["XXBTZ#{currency}"]

        bid = result.fetch('b').first.to_f
        ask = result.fetch('a').first.to_f

        (bid + ask) / 2
      end

      def orders
        @client.closed_orders.dig('result', 'closed')
      end

      def buy(settings)
        currency = settings.fetch('currency')
        price = settings.fetch('price')
        pair = "XBT#{currency}"
        volume = price / current_price(settings)

        response =
          @client
          .add_order(pair: pair, type: 'buy', ordertype: 'market', volume: volume)

        if response.fetch('error').any?
          return Result::Failure.new(
            *@map_errors.call(response.fetch('error'))
          )
        end

        offer_id = response.dig('result', 'txid').first
        order_data = orders[offer_id]
        vol = order_data.fetch('vol').to_f
        cost = order_data.fetch('cost').to_f

        Result::Success.new(
          offer_id: offer_id,
          rate: cost/vol,
          amount: vol
        )
      end

      def sell(settings)
        currency = settings.fetch('currency')
        price = settings.fetch('price')
        pair = "XBT#{currency}"
        volume = price / current_price(settings)

        response =
          @client
          .add_order(pair: pair, type: 'sell', ordertype: 'market', volume: volume)

        if response.fetch('error').any?
          return Result::Failure.new(
            *@map_errors.call(response.fetch('error'))
          )
        end

        offer_id = response.dig('result', 'txid').first
        order_data = orders[offer_id]
        vol = order_data.fetch('vol').to_f
        cost = order_data.fetch('cost').to_f

        Result::Success.new(
          offer_id: offer_id,
          rate: cost/vol,
          amount: vol
        )
      end
    end
  end
end
# rubocop:enable Metrics/LineLength:
