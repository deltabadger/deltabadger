module ExchangeApi
  module Markets
    module Fake
      class Market < BaseMarket
        include ExchangeApi::Clients::Fake

        def initialize
          super
          new_prices
        end

        SUCCESS = true
        MINIMUM_ORDER_VOLUME = 0.001
        MARKET_FEE = 0.1

        def minimum_order_volume(_symbol)
          MINIMUM_ORDER_VOLUME
        end

        def base_decimals(_symbol)
          Result::Success.new(8)
        end

        def minimum_order_parameters(_symbol)
          if SUCCESS
            Result::Success.new(
              minimum: MINIMUM_ORDER_VOLUME,
              minimum_quote: @bid * MINIMUM_ORDER_VOLUME,
              side: BASE,
              fee: 0.1
            )
          else
            Result::Failure.new('Something went wrong!', RECOVERABLE)
          end
        end

        private

        def current_bid_ask_price(_)
          if SUCCESS
            new_prices
            Result::Success.new(BidAskPrice.new(@bid, @ask))
          else
            Result::Failure.new('Something went wrong!', RECOVERABLE)
          end
        end

        def new_prices
          @bid = rand(6000...8000)
          @ask = @bid * (1 + rand * 0.2)
        end
      end
    end
  end
end
