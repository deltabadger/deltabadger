require 'result'

module ExchangeApi
  module Traders
    module Fake
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Fake

        SUCCESS = true
        FETCHED = true

        attr_reader :exchange_name, :bid, :ask

        def initialize(exchange_name, market: ExchangeApi::Markets::Fake::Market.new)
          @exchange_name = exchange_name
          @market = market
        end

        def fetch_order_by_id(_order_id, response_params = nil)
          if SUCCESS
            Result::Success.new(response_params)
          elsif FETCHED
            Result::Failure.new('Something went wrong!')
          else
            Result::Failure.new('Waiting for exchange response', NOT_FETCHED)
          end
        rescue StandardError
          Result::Failure.new('Caught an error while making fake order', RECOVERABLE)
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

        def smart_volume(symbol, price, rate, force_smart_intervals, smart_intervals_value, for_sell = false)
          volume = for_sell ? price.ceil(8) : (price / rate).ceil(8)
          min_volume = @market.minimum_order_volume(symbol)

          smart_intervals_value = min_volume if smart_intervals_value.nil?

          return Result::Success.new([smart_intervals_value, min_volume].max) if force_smart_intervals

          Result::Success.new([min_volume, volume].max)
        end
      end
    end
  end
end
