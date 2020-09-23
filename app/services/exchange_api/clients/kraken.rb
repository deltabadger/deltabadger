# rubocop:disable Metrics/LineLength:
require 'kraken_ruby_client'

module ExchangeApi
  module Clients
    class Kraken < ExchangeApi::Clients::Base
      MIN_TRANSACTION_VOLUME = 0.001

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

      def current_bid_ask_price(currency)
        response = @client.ticker("xbt#{currency}")
        result = response['result']
        key = result.keys.first # The result should contain only one key
        rates = result[key]

        bid = rates.fetch('b').first.to_f
        ask = rates.fetch('a').first.to_f

        Result::Success.new(BidAskPrice.new(bid, ask))
      rescue StandardError
        Result::Failure.new('Could not fetch current price from Kraken', RECOVERABLE)
      end

      def orders
        @client.closed_orders.dig('result', 'closed')
      end

      def market_buy(currency:, price:)
        make_order('market', 'buy', currency, price)
      end

      def market_sell(currency:, price:)
        make_order('market', 'sell', currency, price)
      end

      def limit_buy(currency:, price:, percentage:)
        make_order('limit', 'sell', currency, price, percentage)
      end

      def limit_sell(currency:, price:, percentage:)
        make_order('limit', 'sell', currency, price, percentage)
      end

      private

      def make_order(order_type, offer_type, currency, price, percentage = 0)
        order_params = get_order_params(order_type, offer_type, currency, price, percentage)
        response =
          @client
          .add_order(order_params)

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
        Result::Failure.new('Could not make Kraken order', RECOVERABLE)
      end

      def get_order_params(order_type, offer_type, currency, price, percentage)
        volume_result = smart_volume(offer_type, currency, price, percentage)
        return volume_result unless volume_result.success?

        volume = volume_result.data
        pair = "XBT#{currency}"

        {
          pair: pair,
          type: offer_type,
          ordertype: order_type,
          volume: volume,
          trading_agreement: ('agree' if @options[:german_trading_agreement]),
          price: (current_offer_price(offer_type, currency) if order_type == 'limit')
        }.compact
      end

      def smart_volume(offer_type, currency, price, percentage = 0)
        rate = limit_rate(offer_type, currency, percentage)
        return rate unless rate.success?

        volume = (price / rate.data).ceil(8)
        Result::Success.new([MIN_TRANSACTION_VOLUME, volume].max)
      end
    end
  end
end
# rubocop:enable Metrics/LineLength:
