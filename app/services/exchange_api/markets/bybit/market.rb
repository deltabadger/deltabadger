require 'result'

module ExchangeApi
  module Markets
    module Bybit
      class Market < BaseMarket
        def initialize
          @client = Clients::Bybit.new
        end

        def fetch_all_symbols
          result = @client.instruments_info(category: 'spot')
          return Result::Failure.new('Bybit exchange info is unavailable', RECOVERABLE.to_s) if result.failure?

          items = result.data.dig('result', 'list') || []
          market_symbols = items.map do |symbol_info|
            base = symbol_info['baseCoin']
            quote = symbol_info['quoteCoin']
            MarketSymbol.new(base, quote)
          end

          Result::Success.new(market_symbols)
        rescue StandardError => e
          Result::Failure.new("Bybit exchange info is unavailable. Error: #{e}", RECOVERABLE.to_s)
        end
      end
    end
  end
end
