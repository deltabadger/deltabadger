module ExchangeApi
  module Markets
    module Binance
      class Market < BaseMarket
        include ExchangeApi::Clients::Binance

        PRICE_FILTER = 'PRICE_FILTER'.freeze
        MIN_NOTIONAL = 'MIN_NOTIONAL'.freeze

        def minimum_order_price(symbol)
          symbol_info = fetch_symbol_info(symbol)
          return symbol_info unless symbol_info.success?

          minimum_price = get_minimum_quote_notional(symbol_info.data)
          return minimum_price unless symbol_info.success?

          minimum_price.data
        end

        private

        def current_bid_ask_price(symbol)
          request = unsigned_client.get('ticker/bookTicker', { symbol: symbol }, {})
          response = JSON.parse(request.body)

          bid = response.fetch('bidPrice').to_f
          ask = response.fetch('askPrice').to_f

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new('Could not fetch current price from Binance')
        end

        def fetch_symbol_info(symbol)
          request = unsigned_client.get('exchangeInfo')
          response = JSON.parse(request.body)
          symbols = response['symbols']
          found_symbol = symbols.find do |symbol_info|
            symbol_info['symbol'] == symbol
          end
          return Result::Failure.new('Invalid ticker symbol') if found_symbol.nil?

          Result::Success.new(found_symbol)
        end

        def get_minimum_quote_notional(symbol_info)
          filters = symbol_info['filters']
          notional_filter = filters.find do |filter|
            filter['filterType'] == MIN_NOTIONAL
          end
          return 0 if notional_filter.nil?

          min_price = notional_filter['minPrice']
          min_price.to_f
        end
      end
    end
  end
end
