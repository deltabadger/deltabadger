require 'result'
module ExchangeApi
  module Markets
    module Binance
      class Market < BaseMarket
        include ExchangeApi::Clients::Binance

        def initialize(url_base:)
          @base_client = base_client(url_base)
          @caching_client = caching_client(url_base)
        end

        PRICE_FILTER = 'PRICE_FILTER'.freeze
        MIN_NOTIONAL = 'MIN_NOTIONAL'.freeze
        LOT_SIZE = 'LOT_SIZE'.freeze

        def minimum_order_price(symbol)
          minimum_notional = min_notional(symbol)
          return minimum_notional unless minimum_notional.success?

          minimum_lot = min_lot_size(symbol)
          return minimum_lot unless minimum_lot.success?

          current_ask = current_ask_price(symbol)
          return current_ask unless current_ask.success?

          minimum_price = [(minimum_lot.data * current_ask.data * 1.1).to_d, minimum_notional.data].max
          Result::Success.new(minimum_price)
        end

        def minimum_order_volume(symbol)
          min_lot = min_lot_size(symbol)
          return min_lot unless min_lot.success?

          Result::Success.new(min_lot.data)
        end

        def base_decimals(symbol)
          symbol_info = fetch_symbol_info(symbol)
          return symbol_info unless symbol_info.success?

          Result::Success.new(symbol_info.data['baseAssetPrecision'])
        end

        def base_step_size(symbol)
          step_size = lot_step_size(symbol)
          return step_size unless step_size.success?

          Result::Success.new(step_size.data.to_d)
        end

        def quote_tick_size(symbol)
          tick_size = price_tick_size(symbol)
          return tick_size unless tick_size.success?

          Result::Success.new(tick_size.data.to_d)
        end

        def quote_decimals(symbol)
          tick_size = quote_tick_size(symbol)
          return tick_size unless tick_size.success?

          tick_size_decimals = tick_size.data.to_s.split('.').last.size
          Result::Success.new(tick_size_decimals)
        end

        def quote_tick_size_decimals(symbol)
          symbol_info = fetch_symbol_info(symbol)
          return symbol_info unless symbol_info.success?

          Result::Success.new(symbol_info.data['quoteAssetPrecision'])
        end

        def base_tick_size_decimals(symbol)
          symbol_info = fetch_symbol_info(symbol)
          return symbol_info unless symbol_info.success?

          Result::Success.new(symbol_info.data['baseAssetPrecision'])
        end

        def fetch_all_symbols
          request = @base_client.get('exchangeInfo')
          exchange_info = JSON.parse(request.body)
          symbols = exchange_info['symbols']

          market_symbols = symbols.map do |symbol_info|
            base = symbol_info['baseAsset']
            quote = symbol_info['quoteAsset']
            MarketSymbol.new(base, quote)
          end
          Result::Success.new(market_symbols)
        rescue StandardError
          Result::Failure.new('Binance exchange info is unavailable', RECOVERABLE)
        end

        def minimum_order_parameters(symbol)
          minimum = minimum_order_price(symbol)
          return minimum unless minimum.success?

          Result::Success.new(
            minimum: minimum.data,
            minimum_quote: minimum.data,
            side: QUOTE
          )
        end

        private

        def current_bid_ask_price(symbol)
          request = @base_client.get('ticker/bookTicker', { symbol: symbol }, {})
          response = JSON.parse(request.body)

          bid = response.fetch('bidPrice').to_d
          ask = response.fetch('askPrice').to_d

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new('Could not fetch current price from Binance', RECOVERABLE)
        end

        def find_symbol_in_exchange_info(symbol, exchange_info)
          symbols = exchange_info['symbols']
          found_symbol = symbols.find do |symbol_info|
            symbol_info['symbol'] == symbol
          end
          if found_symbol.present?
            Result::Success.new(found_symbol)
          else
            Result::Failure.new('Invalid ticker symbol')
          end
        end

        def exchange_info_cache_key(symbol)
          "binance_exchange_info_#{symbol}"
        end

        def min_price(symbol)
          symbol_info = fetch_symbol_info(symbol)
          return symbol_info unless symbol_info.success?

          price_filter = find_filter(symbol_info.data, PRICE_FILTER)
          return Result::Success.new(0) if price_filter.nil?

          min_price = price_filter['minPrice']
          Result::Success.new(min_price.to_d)
        end

        def price_tick_size(symbol)
          symbol_info = fetch_symbol_info(symbol)
          return symbol_info unless symbol_info.success?

          price_filter = find_filter(symbol_info.data, PRICE_FILTER)
          return Result::Success.new(0) if price_filter.nil?

          tick_size = price_filter['tickSize']
          Result::Success.new(tick_size)
        end

        def min_lot_size(symbol)
          symbol_info = fetch_symbol_info(symbol)
          return symbol_info unless symbol_info.success?

          lot_filter = find_filter(symbol_info.data, LOT_SIZE)
          return Result::Success.new(0) if lot_filter.nil?

          min_lot = lot_filter['minQty']
          Result::Success.new(min_lot.to_d)
        end

        def lot_step_size(symbol)
          symbol_info = fetch_symbol_info(symbol)
          return symbol_info unless symbol_info.success?

          lot_filter = find_filter(symbol_info.data, LOT_SIZE)
          return Result::Success.new(0) if lot_filter.nil?

          step_size = lot_filter['stepSize']
          Result::Success.new(step_size)
        end

        def min_notional(symbol)
          symbol_info = fetch_symbol_info(symbol)
          return symbol_info unless symbol_info.success?

          notional_filter = find_filter(symbol_info.data, MIN_NOTIONAL)
          return Result::Success.new(0) if notional_filter.nil?

          min_notional = notional_filter['minNotional']
          Result::Success.new(min_notional.to_d)
        end

        def find_filter(symbol_info, target_filter)
          filters = symbol_info['filters']
          filters.find do |filter|
            filter['filterType'] == target_filter
          end
        end

        def fetch_symbol_info(symbol)
          request = @caching_client.get('exchangeInfo')
          response = JSON.parse(request.body)
          found_symbol = find_symbol_in_exchange_info(symbol, response)
          return found_symbol unless found_symbol.success?

          Result::Success.new(found_symbol.data)
        end
      end
    end
  end
end
