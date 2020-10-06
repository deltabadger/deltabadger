module ExchangeApi
  module Markets
    module Binance
      class Market < BaseMarket
        include ExchangeApi::Clients::Binance

        private

        def current_bid_ask_price(currency)
          symbol = "BTC#{currency.upcase}"
          request = unsigned_client.get('ticker/bookTicker', { symbol: symbol }, {})
          response = JSON.parse(request.body)

          bid = response.fetch('bidPrice').to_f
          ask = response.fetch('askPrice').to_f

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new('Could not fetch current price from Binance')
        end
      end
    end
  end
end
