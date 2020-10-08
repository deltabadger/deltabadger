module ExchangeApi
  module Markets
    module Kraken
      class Market < BaseMarket
        include ExchangeApi::Clients::Kraken

        def minimum_order_volume(symbol)
          symbol_info = fetch_symbol(symbol)
          return symbol_info unless symbol_info.success?

          symbol_info.data['ordermin'].to_f
        end

        def base_decimals(symbol)
          symbol_info = fetch_symbol(symbol)
          return symbol_info unless symbol_info.success?

          symbol_info.data['lot_decimals']
        end

        def quote_decimals(symbol)
          symbol_info = fetch_symbol(symbol)
          return symbol_info unless symbol_info.success?

          symbol_info.data['pair_decimals']
        end

        private

        def fetch_symbol(symbol)
          response = @client.asset_pairs(symbol)
          found_symbol = response['result'].first[1] # Value of the only hash element
          Result::Success.new(found_symbol)
        rescue StandardError
          Result::Failure.new(['Could not fetch chosen symbol from Kraken', RECOVERABLE])
        end

        def current_bid_ask_price(symbol)
          response = @client.ticker(symbol)
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
