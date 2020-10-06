module ExchangeApi
  module Markets
    module Bitbay
      class Market < BaseMarket
        include ExchangeApi::Clients::Bitbay

        private

        def current_bid_ask_price(currency)
          url = "https://bitbay.net/API/Public/BTC#{currency}/ticker.json"
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
