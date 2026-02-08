require 'result'

module ExchangeApi
  module Markets
    module Bitvavo
      class Market < BaseMarket
        def initialize
          @client = Clients::Bitvavo.new
        end

        def symbol(base, quote)
          "#{base}-#{quote}"
        end

        def fetch_all_symbols
          result = @client.markets
          return Result::Failure.new('Bitvavo exchange info is unavailable', RECOVERABLE.to_s) if result.failure?

          market_symbols = result.data.map do |market_info|
            market = market_info['market']
            base, quote = market.split('-')
            MarketSymbol.new(base, quote)
          end

          Result::Success.new(market_symbols)
        rescue StandardError => e
          Result::Failure.new("Bitvavo exchange info is unavailable. Error: #{e}", RECOVERABLE.to_s)
        end
      end
    end
  end
end
