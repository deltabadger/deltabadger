module ExchangeApi
  module Markets
    class BaseMarket
      RECOVERABLE = { data: { recoverable: true }.freeze }.freeze
      BASE = 'base'.freeze
      QUOTE = 'quote'.freeze

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

      def fetch_all_symbols
        raise NotImplementedError
      end

      def all_symbols(cache_key, expires_in = 1.hour)
        return Result::Success.new(Rails.cache.read(cache_key)) if Rails.cache.exist?(cache_key)

        symbols = fetch_all_symbols
        return symbols unless symbols.success?

        Rails.cache.write(cache_key, symbols.data, expires_in: expires_in)
        Result::Success.new(symbols.data)
      end

      def base_decimals(_symbol)
        raise NotImplementedError
      end

      def quote_decimals(_symbol)
        raise NotImplementedError
      end

      def minimum_order_parameters(_symbol)
        raise NotImplementedError
      end

      def fee(_symbol)
        Result::Success.new('0.00')
      end

      private

      def current_bid_ask_price(_symbol)
        raise NotImplementedError
      end
    end
  end
end
