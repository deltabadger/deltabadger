module ExchangeApi
  module Markets
    module Kraken
      class Market < BaseMarket
        include ExchangeApi::Clients::Kraken

        def minimum_order_volume(symbol)
          response = @client.asset_pairs(symbol)
          symbol_info = response['result'].first[1] # Value of the only hash element
          symbol_info['ordermin'].to_f
        end

        private

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
