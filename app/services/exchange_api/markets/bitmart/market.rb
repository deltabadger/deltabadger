require 'result'

module ExchangeApi
  module Markets
    module Bitmart
      class Market < BaseMarket
        def initialize
          @client = Clients::Bitmart.new
        end

        def fetch_all_symbols
          result = @client.get_symbols
          return Result::Failure.new('Bitmart exchange info is unavailable', RECOVERABLE.to_s) if result.failure?

          items = result.data.dig('data', 'symbols') || []
          market_symbols = items.map do |symbol_info|
            base = symbol_info['base_currency']
            quote = symbol_info['quote_currency']
            MarketSymbol.new(base, quote)
          end

          Result::Success.new(market_symbols)
        rescue StandardError => e
          Result::Failure.new("Bitmart exchange info is unavailable. Error: #{e}", RECOVERABLE.to_s)
        end
      end
    end
  end
end
