# rubocop:disable Metrics/LineLength:
require 'kraken_ruby_client'

module ExchangeApi
  module Clients
    class Kraken < ExchangeApi::Clients::Base
      def initialize(api_key:, api_secret:, map_errors: ExchangeApi::MapErrors::Kraken.new, options: {})
        @client =
          ::Kraken::Client.new(api_key: api_key, api_secret: api_secret)
        @map_errors = map_errors
        @options = options
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
        puts 'Buying on kraken'
        make_order('buy', settings)
      end

      def sell(settings)
        puts 'selling on kraken'
        make_order('sell', settings)
      end

      private

      def make_order(offer_type, settings) # rubocop:disable Metrics/MethodLength
        currency = settings.fetch('currency')
        # price = settings.fetch('price')
        volume = 0.002
        pair = "XBT#{currency}"

        request_params = {
          pair: pair,
          type: offer_type,
          ordertype: 'market',
          volume: volume
        }
        request_params = request_params.merge(trading_agreement: 'agree') if @options[:german_trading_agreement]
        response =
          @client
          .add_order(request_params)

        if response.fetch('error').any?
          return Result::Failure.new(
            *@map_errors.call(response.fetch('error'))
          )
        end

        offer_id = response.dig('result', 'txid').first
        order_data = orders[offer_id]
        rate = order_data.fetch('price').to_f

        Result::Success.new(
          offer_id: offer_id,
          rate: rate,
          amount: volume
        )
      end
    end
  end
end
# rubocop:enable Metrics/LineLength:
