module ExchangeApi
  module Clients
    class BaseTrader < BaseClient
      def buy
        raise NotImplementedError
      end

      def sell
        raise NotImplementedError
      end

      def current_bid_ask_price(_currency)
        raise NotImplementedError
      end

      def current_price(currency)
        result = current_bid_ask_price(currency)
        return result unless result.success?

        price = result.data
        Result::Success.new((price.bid + price.ask) / 2)
      end

      def current_bid_price(currency)
        result = current_bid_ask_price(currency)
        return result unless result.success?

        Result::Success.new(result.data.bid)
      end

      def current_ask_price(currency)
        result = current_bid_ask_price(currency)
        return result unless result.success?

        Result::Success.new(result.data.ask)
      end
    end
  end
end

