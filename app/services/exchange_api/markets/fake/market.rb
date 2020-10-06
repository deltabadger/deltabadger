module ExchangeApi
  module Markets
    module Fake
      class Market < BaseMarket
        include ExchangeApi::Clients::Fake

        SUCCESS = true
        MINIMUM_ORDER_VOLUME = 0.001

        def minimum_order_volume(_symbol)
          MINIMUM_ORDER_VOLUME
        end

        private

        def current_bid_ask_price(_)
          if SUCCESS
            new_prices
            Result::Success.new(BidAskPrice.new(bid, ask))
          else
            Result::Failure.new('Something went wrong!', RECOVERABLE)
          end
        end
      end
    end
  end
end
