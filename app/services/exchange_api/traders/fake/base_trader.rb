require 'result'

module ExchangeApi
  module Traders
    module Fake
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Fake

        SUCCESS = true

        attr_reader :exchange_name, :bid, :ask

        def initialize(exchange_name, market: ExchangeApi::Markets::Fake::Market.new)
          @exchange_name = exchange_name
          @market = market
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

        def smart_volume(symbol, price, rate, force)
          volume = (price / rate).ceil(8)
          min_volume = @market.minimum_order_volume(symbol)
          Result::Success.new(force ? min_volume : [min_volume, volume].max)
        end
      end
    end
  end
end
