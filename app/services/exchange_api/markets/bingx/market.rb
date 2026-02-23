require 'result'

module ExchangeApi
  module Markets
    module Bingx
      class Market < BaseMarket
        def initialize
          @client = Clients::Bingx.new
        end

        def fetch_all_symbols
          result = @client.get_symbols
          return Result::Failure.new('BingX exchange info is unavailable', RECOVERABLE.to_s) if result.failure?

          items = result.data.dig('data', 'symbols') || []
          market_symbols = items.map do |symbol_info|
            base = symbol_info['baseAsset']
            quote = symbol_info['quoteAsset']
            MarketSymbol.new(base, quote)
          end

          Result::Success.new(market_symbols)
        rescue StandardError => e
          Result::Failure.new("BingX exchange info is unavailable. Error: #{e}", RECOVERABLE.to_s)
        end
      end
    end
  end
end
