module ExchangeApi
  module Markets
    module Binance
      class Market < BaseMarket
        include ExchangeApi::Clients::Binance

        PRICE_FILTER = 'PRICE_FILTER'.freeze
        MIN_NOTIONAL = 'MIN_NOTIONAL'.freeze
        LOT_SIZE = 'LOT_SIZE'.freeze

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

        def get_minimum_order_price(symbol_info)
          price_filter = find_filter(symbol_info, PRICE_FILTER)
          return 0 if price_filter.nil?

          min_price = price_filter['minPrice']
          min_price.to_f
        end

        def get_minimum_lot_size(symbol_info)
          lot_filter = find_filter(symbol_info, LOT_SIZE)
          return 0 if lot_filter.nil?

          min_lot = lot_filter['minQty']
          min_lot.to_f
        end

        def get_minimum_quote_notional(symbol_info)
          notional_filter = find_filter(symbol_info, MIN_NOTIONAL)
          return 0 if notional_filter.nil?

          min_notional = notional_filter['minNotional']
          min_notional.to_f
        end

        def find_filter(symbol_info, target_filter)
          filters = symbol_info['filters']
          filters.find do |filter|
            filter['filterType'] == target_filter
          end
        end
      end
    end
  end
end
