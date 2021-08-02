require 'result'

module ExchangeApi
  module Markets
    module Ftx
      class Market < BaseMarket
        include ExchangeApi::Clients::Ftx

        def initialize
          @base_client = base_client(API_URL)
          @caching_client = caching_client(API_URL)
        end

        def fetch_all_symbols
          request = @caching_client.get('/api/markets')
          response = JSON.parse(request.body)
          market_symbols = response.fetch('result').map do |symbol_info|
            base = get_base(symbol_info)
            quote = get_quote(symbol_info)
            next if base.blank? || quote.blank?

            MarketSymbol.new(base, quote)
          end
          Result::Success.new(market_symbols.compact)
        rescue StandardError
          Result::Failure.new("Couldn't fetch FTX symbols", RECOVERABLE)
        end

        def minimum_order_price(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data['priceIncrement'].to_f)
        end

        def minimum_base_size(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data['sizeIncrement'].to_f)
        end

        def base_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          result = GetNumberOfDecimalPoints.call(response.data['sizeIncrement'])

          Result::Success.new(result)
        rescue StandardError
          Result::Failure.new("Couldn't fetch FTX symbol details", RECOVERABLE)
        end

        def quote_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          result = GetNumberOfDecimalPoints.call(response.data['priceIncrement'])

          Result::Success.new(result)
        rescue StandardError
          Result::Failure.new("Couldn't fetch FTX symbol details", RECOVERABLE)
        end

        def symbol(base, quote)
          return "#{base}-#{quote}" if future?(quote)

          "#{base}/#{quote}"
        end

        def minimum_order_parameters(symbol)
          minimum = minimum_base_size(symbol)
          return minimum unless minimum.success?

          ask = current_ask_price(symbol)
          return ask unless ask.success?

          Result::Success.new(
            minimum: minimum.data,
            minimum_quote: minimum.data * ask.data,
            side: BASE
          )
        end

        private

        def fetch_symbol(symbol, cached = true)
          client = get_client(cached)
          request = client.get("/api/markets/#{symbol}")
          response = JSON.parse(request.body)
          Result::Success.new(response.fetch('result'))
        rescue StandardError
          Result::Failure.new("Couldn't fetch chosen symbol from FTX", RECOVERABLE)
        end

        def get_client(cached = true)
          cached ? @caching_client : @base_client
        end

        def current_bid_ask_price(symbol)
          response = fetch_symbol(symbol, false)
          return response unless response.success?

          response = response.data
          bid = response['bid'].to_f
          ask = response['ask'].to_f

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new("Couldn't fetch bid/ask price from FTX", RECOVERABLE)
        end

        def get_quote(symbol_info)
          return symbol_info.fetch('name').split('-')[1] if symbol_info.fetch('type') == 'future'

          symbol_info.fetch('name').split('/')[1]
        end

        def get_base(symbol_info)
          return symbol_info.fetch('name').split('-')[0] if symbol_info.fetch('type') == 'future'

          symbol_info.fetch('name').split('/')[0]
        end

        def future?(quote)
          quote == 'PERP' || /\A\d+\z/.match(quote)
        end
      end
    end
  end
end
