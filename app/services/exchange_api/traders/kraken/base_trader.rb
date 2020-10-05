# rubocop:disable Metrics/LineLength:
require 'result'

module ExchangeApi
  module Traders
    module Kraken
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Kraken

        MIN_TRANSACTION_VOLUME = 0.001

        def initialize(api_key:, api_secret:, map_errors: ExchangeApi::MapErrors::Kraken.new, options: {})
          @client = get_client(api_key, api_secret)
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

          order = response.dig('result', 'descr', 'order')
          offer_id = order['offerId']
          rate = order['price'].to_f

          Result::Success.new(
            offer_id: offer_id,
            rate: rate,
            amount: order_params.volume
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
          volume = (price / rate).ceil(8)
          Result::Success.new([MIN_TRANSACTION_VOLUME, volume].max)
        end
      end
    end
  end
end
# rubocop:enable Metrics/LineLength:
