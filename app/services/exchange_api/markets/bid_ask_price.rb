module ExchangeApi
  module Traders
    class BidAskPrice
      attr_reader :bid, :ask

      def initialize(bid, ask)
        @bid = bid
        @ask = ask
      end
    end
  end
end
