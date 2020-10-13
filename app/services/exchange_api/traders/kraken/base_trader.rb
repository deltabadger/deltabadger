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

        private

        def place_order(order_params)
          response = @client.add_order(order_params)
          return error_to_failure(response.fetch('error')) if response.fetch('error').any?

          result = parse_response(response)
          Result::Success.new(result)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not make Kraken order', RECOVERABLE)
        end

        def parse_response(response)
          created_order = response.fetch('result')
          offer_id = created_order.fetch('txid').first
          order_data = orders.fetch(offer_id)
          rate = if order_data.fetch('status') == 'open'
                   order_data.fetch('descr').fetch('price').to_f
                 else # closed
                   order_data.fetch('price').to_f
                 end
          amount = order_data.fetch('vol').to_f
          {
            offer_id: offer_id,
            rate: rate,
            amount: amount
          }
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

        def orders
          raise NotImplementedError
        end
      end
    end
  end
end
# rubocop:enable Metrics/LineLength:
