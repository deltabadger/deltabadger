# rubocop:disable Metrics/LineLength:
require 'kraken_ruby_client'

module ExchangeApi
  module Clients
    class Kraken < ExchangeApi::Clients::Base
      MIN_TRANSACTION_VOLUME = 0.002

      def initialize(api_key:, api_secret:, map_errors: ExchangeApi::MapErrors::Kraken.new, options: {})
        @client =
          ::Kraken::Client.new(api_key: api_key, api_secret: api_secret)
        @map_errors = map_errors
        @options = options
      end

      def validate_credentials
        response = @client.balance
        response['error'].none?
      rescue StandardError
        false
      end

      def current_bid_ask_price(settings)
        currency = settings.fetch('currency')

        response = @client.ticker("xbt#{currency}")
        result = response['result']
        key = result.keys.first # The result should contain only one key
        rates = result[key]

        bid = rates.fetch('b').first.to_f
        ask = rates.fetch('a').first.to_f

        Result::Success.new(BidAskPrice.new(bid, ask))
      rescue StandardError
        Result::Failure.new('Could not fetch current price from Kraken', data: { recoverable: true })
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

        volume_result = smart_volume(offer_type, settings)
        return volume_result unless volume_result.success?

        volume = volume_result.data
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

        return error_to_failure(response.fetch('error')) if response.fetch('error').any?

        offer_id = response.dig('result', 'txid').first
        order_data = orders[offer_id]
        rate = order_data.fetch('price').to_f

        Result::Success.new(
          offer_id: offer_id,
          rate: rate,
          amount: volume
        )
      rescue StandardError
        Result::Failure.new('Could not make Kraken order', data: { recoverable: true })
      end

      def smart_volume(offer_type, settings)
        rate = if offer_type == 'sell'
                 current_bid_price(settings)
               else
                 current_ask_price(settings)
               end
        return rate unless rate.success?

        price = settings.fetch('price').to_f
        volume = price / rate.data

        Result::Success.new([MIN_TRANSACTION_VOLUME, volume].max)
      end

      def error_to_failure(error)
        mapped_error = @map_errors.call(error)
        Result::Failure.new(
          *mapped_error.message, data: { retry: mapped_error.recoverable }
        )
      end
    end
  end
end
# rubocop:enable Metrics/LineLength:
