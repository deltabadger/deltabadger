module ExchangeApi
  module Markets
    module Bitbay
      class Market < BaseMarket
        include ExchangeApi::Clients::Bitbay

        def minimum_order_price(symbol)
          url = "https://api.bitbay.net/rest/trading/ticker/#{symbol}"
          response = JSON.parse(Faraday.get(url))
          minimum_quote_price = response.dig('ticker', 'market', 'second', 'minOffer')
          minimum_quote_price.to_f
        end

        def symbol(base, quote)
          "#{base}-#{quote}"
        end

        private

        def current_bid_ask_price(symbol)
          url = "https://bitbay.net/API/Public/#{symbol}/ticker.json"
          response = JSON.parse(Faraday.get(url, {}, headers(@api_key, @api_secret, '')).body)

          bid = response.fetch('bid').to_f
          ask = response.fetch('ask').to_f

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new('Could not fetch current price from Bitbay', RECOVERABLE)
        end
      end
    end
  end
end
