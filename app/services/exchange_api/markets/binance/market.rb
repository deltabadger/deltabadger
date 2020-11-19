require 'result'
module ExchangeApi
  module Markets
    module Binance
      class Market < BaseMarket
        include ExchangeApi::Clients::Binance

        PRICE_FILTER = 'PRICE_FILTER'.freeze
        MIN_NOTIONAL = 'MIN_NOTIONAL'.freeze
        LOT_SIZE = 'LOT_SIZE'.freeze

        def minimum_order_price(symbol)
          minimum_price = min_notional(symbol)
          return minimum_price unless minimum_price.success?

          Result::Success.new(minimum_price.data)
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

          Result::Success.new(step_size.data.to_f)
        end

        def quote_decimals(symbol)
          symbol_info = fetch_symbol_info(symbol)
          return symbol_info unless symbol_info.success?

          Result::Success.new(symbol_info.data['quoteAssetPrecision'])
        end

        def quote_tick_size(symbol)
          tick_size = price_tick_size(symbol)
          return tick_size unless tick_size.success?

          Result::Success.new(tick_size.data.to_f)
        end

        def all_symbols
          request = unsigned_client.get('exchangeInfo')
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

        private

        def current_bid_ask_price(symbol)
          request = unsigned_client.get('ticker/bookTicker', { symbol: symbol }, {})
          response = JSON.parse(request.body)

          bid = response.fetch('bidPrice').to_f
          ask = response.fetch('askPrice').to_f

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
          Result::Success.new(min_price.to_f)
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
          Result::Success.new(min_lot.to_f)
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
          Result::Success.new(min_notional.to_f)
        end

        def find_filter(symbol_info, target_filter)
          filters = symbol_info['filters']
          filters.find do |filter|
            filter['filterType'] == target_filter
          end
        end

        def fetch_symbol_info(symbol)
          cache_key = exchange_info_cache_key(symbol)
          return Result::Success.new(Rails.cache.read(cache_key)) if Rails.cache.exist?(cache_key)

          request = unsigned_client.get('exchangeInfo')
          response = JSON.parse(request.body)
          found_symbol = find_symbol_in_exchange_info(symbol, response)
          return found_symbol unless found_symbol.success?

          Rails.cache.write(cache_key, found_symbol.data, expires_in: 1.hour)
          Result::Success.new(found_symbol.data)
        end
      end
    end
  end
end
