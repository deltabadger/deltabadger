require 'result'

module ExchangeApi
  module Clients
    module Fake
      class Base < ExchangeApi::Clients::BaseTrader
        MIN_TRANSACTION_VOLUME = 0.002

        SUCCESS = true
        # SUCCESS = false

        attr_reader :exchange_name, :bid, :ask

        def initialize(exchange_name)
          @exchange_name = exchange_name
          new_prices
        end

        def current_bid_ask_price(_)
          if SUCCESS
            new_prices
            Result::Success.new(BidAskPrice.new(bid, ask))
          else
            Result::Failure.new('Something went wrong!', RECOVERABLE)
          end
        end

        private

        def place_order(order_params)
          if SUCCESS
            Result::Success.new(
              order_params
            )
          else
            Result::Failure.new('Something went wrong!')
          end
        rescue StandardError
          Result::Failure.new('Caught an error while making fake order', RECOVERABLE)
        end

        def common_order_params
          {
            offer_id: SecureRandom.uuid
          }
        end

        def smart_volume(price, rate)
          volume = (price / rate).ceil(8)
          Result::Success.new([MIN_TRANSACTION_VOLUME, volume].max)
        end

        def new_prices
          @bid = rand(6000...8000)
          @ask = @bid * (1 + rand * 0.2)
        end
      end
    end
  end
end
