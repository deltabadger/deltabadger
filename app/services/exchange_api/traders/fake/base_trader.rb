require 'result'

module ExchangeApi
  module Traders
    module Fake
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Fake

        SUCCESS = false
        FETCHED = false

        attr_reader :exchange_name, :bid, :ask

        def initialize(exchange_name, market: ExchangeApi::Markets::Fake::Market.new)
          @exchange_name = exchange_name
          @market = market
        end

        def fetch_order_by_id(order_id)
          if SUCCESS
            Result::Success.new(
              offer_id: order_id,
              amount: 0.00001, # TODO, change to real values
              rate: 25000
            )
          elsif FETCHED
            Result::Failure.new('Something went wrong!')
          else
            Result::Failure.new('Waiting for exchange response', NOT_FETCHED)
          end
        rescue StandardError => e
          Result::Failure.new('Caught an error while making fake order', RECOVERABLE)
        end

        private

        def place_order(order_params)
          if true
            Result::Success.new(
              offer_id: order_params.fetch(:offer_id)
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

        def smart_volume(symbol, price, rate, force_smart_intervals)
          volume = (price / rate).ceil(8)
          min_volume = @market.minimum_order_volume(symbol)
          return Result::Success.new(min_volume) if force_smart_intervals

          Result::Success.new([min_volume, volume].max)
        end
      end
    end
  end
end
