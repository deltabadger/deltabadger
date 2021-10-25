require 'result'

module ExchangeApi
  module Markets
    module Kraken
      class Market < BaseMarket
        include ExchangeApi::Clients::Kraken

        def initialize
          @base_client = get_base_client('anything', 'anything')
          @caching_client = get_caching_client('anything', 'anything')
        end

        def minimum_order_volume(symbol)
          symbol_info = fetch_symbol(symbol)
          return symbol_info unless symbol_info.success?

          Result::Success.new(symbol_info.data['ordermin'].to_f)
        end

        def fetch_all_symbols
          symbols = @caching_client.asset_pairs
          alt_names = altname_symbols
          market_symbols = symbols['result'].map do |_symbol, data|
            base = alt_names[data['base']]
            quote = alt_names[data['quote']]
            MarketSymbol.new(base, quote)
          end
          Result::Success.new(market_symbols)
        rescue StandardError
          Result::Failure.new('Kraken exchange info is unavailable', RECOVERABLE)
        end

        def base_decimals(symbol)
          symbol_info = fetch_symbol(symbol)
          return symbol_info unless symbol_info.success?

          Result::Success.new(symbol_info.data['lot_decimals'])
        end

        def quote_decimals(symbol)
          symbol_info = fetch_symbol(symbol)
          return symbol_info unless symbol_info.success?

          Result::Success.new(symbol_info.data['pair_decimals'])
        end

        def minimum_order_parameters(symbol)
          minimum = minimum_order_volume(symbol)
          return minimum unless minimum.success?

          ask = current_ask_price(symbol)
          return ask unless ask.success?

          fee = fee(symbol)
          return fee unless fee.success?

          Result::Success.new(
            minimum: minimum.data,
            minimum_quote: minimum.data * ask.data,
            side: BASE,
            fee: fee.data
          )
        end

        def fee(symbol)
          symbol_info = fetch_symbol(symbol)
          return symbol_info unless symbol_info.success?

          Result::Success.new(symbol_info.data['fees'][0][1])
        end

        private

        def altname_symbols
          symbols = @caching_client.assets
          Hash[symbols['result'].map.collect { |name, data| [name, data['altname']] }]
        end

        def fetch_symbol(symbol)
          response = @caching_client.asset_pairs(symbol)
          found_symbol = response['result'].first[1] # Value of the only hash element
          Result::Success.new(found_symbol)
        rescue StandardError
          Result::Failure.new('Could not fetch chosen symbol from Kraken', RECOVERABLE)
        end

        def current_bid_ask_price(symbol)
          response = @base_client.ticker(symbol)
          result = response['result']
          key = result.keys.first # The result should contain only one key
          rates = result[key]

          bid = rates.fetch('b').first.to_f
          ask = rates.fetch('a').first.to_f

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new('Could not fetch current price from Kraken', RECOVERABLE)
        end
      end
    end
  end
end
