require 'result'

module ExchangeApi
  module Markets
    module Bitrue
      class Market < BaseMarket
        def initialize
          @client = Clients::Bitrue.new
        end

        def fetch_all_symbols
          result = @client.exchange_information
          return Result::Failure.new('Bitrue exchange info is unavailable', RECOVERABLE.to_s) if result.failure?

          symbols = result.data['symbols']
          market_symbols = symbols.map do |symbol_info|
            base = symbol_info['baseAsset']
            quote = symbol_info['quoteAsset']
            MarketSymbol.new(base, quote)
          end

          Result::Success.new(market_symbols)
        rescue StandardError => e
          Result::Failure.new("Bitrue exchange info is unavailable. Error: #{e}", RECOVERABLE.to_s)
        end
      end
    end
  end
end
