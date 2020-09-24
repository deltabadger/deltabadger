# rubocop:disable Metrics/LineLength:
require 'result'
require 'kraken_ruby_client'

module ExchangeApi
  module Clients
    module Kraken
      class BaseTrader < ExchangeApi::Clients::BaseTrader
        MIN_TRANSACTION_VOLUME = 0.001

        def initialize(api_key:, api_secret:, map_errors: ExchangeApi::MapErrors::Kraken.new, options: {})
          @client = ::Kraken::Client.new(api_key: api_key, api_secret: api_secret)
          @map_errors = map_errors
          @options = options
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

        private

        def place_order(order_params)
          response = @client.add_order(order_params)

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

        def common_order_params(currency)
          pair = "XBT#{currency}"
          {
            pair: pair,
            trading_agreement: ('agree' if @options[:german_trading_agreement])
          }.compact
        end

        def smart_volume(price, rate)
          volume = (price / rate.data).ceil(8)
          Result::Success.new([MIN_TRANSACTION_VOLUME, volume].max)
        end
      end
    end
  end
end
# rubocop:enable Metrics/LineLength:
