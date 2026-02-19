require 'result'

module ExchangeApi
  module Markets
    module Hyperliquid
      class Market < BaseMarket
        def initialize
          @client = Clients::Hyperliquid.new
        end

        def fetch_all_symbols
          result = @client.spot_meta
          return Result::Failure.new('Hyperliquid exchange info is unavailable', RECOVERABLE.to_s) if result.failure?

          tokens = result.data['tokens']
          universe = result.data['universe']
          token_map = tokens.each_with_object({}) { |t, h| h[t['index']] = t }

          market_symbols = universe.map do |pair|
            base_token = token_map[pair['tokens'][0]]
            quote_token = token_map[pair['tokens'][1]]
            next unless base_token && quote_token

            MarketSymbol.new(base_token['name'], quote_token['name'])
          end.compact

          Result::Success.new(market_symbols)
        rescue StandardError => e
          Result::Failure.new("Hyperliquid exchange info is unavailable. Error: #{e}", RECOVERABLE.to_s)
        end
      end
    end
  end
end
