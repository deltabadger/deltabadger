module ExchangeApi
  module Markets
    class BaseMarket
      def current_price(symbol)
        result = current_bid_ask_price(symbol)
        return result unless result.success?

        price = result.data
        Result::Success.new((price.bid + price.ask) / 2)
      end

      def current_bid_price(symbol)
        result = current_bid_ask_price(symbol)
        return result unless result.success?

        Result::Success.new(result.data.bid)
      end

      def current_ask_price(symbol)
        result = current_bid_ask_price(symbol)
        return result unless result.success?

        Result::Success.new(result.data.ask)
      end

      def symbol(base, quote)
        "#{base}#{quote}"
      end

      def base_decimals(_symbol)
        raise NotImplementedError
      end

      def quote_decimals(_symbol)
        raise NotImplementedError
      end

      private

      def current_bid_ask_price(_symbol)
        raise NotImplementedError
      end
    end
  end
end
