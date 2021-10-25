require 'result'

module ExchangeApi
  module Markets
    module Bitfinex
      class Market < BaseMarket
        include ExchangeApi::Clients::Bitfinex

        def initialize
          @base_client = base_client(PUBLIC_API_URL)
          @caching_client = caching_client(PUBLIC_API_URL)
        end

        def fetch_all_symbols
          response = fetch_symbols
          return response unless response.success?

          market_symbols = response.data[0].reject { |s| skip_symbol?(s) }.map do |symbol_info|
            base = get_base(symbol_info)
            quote = get_quote(symbol_info)

            MarketSymbol.new(base, quote)
          end
          Result::Success.new(market_symbols)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Bitfinex symbols", RECOVERABLE)
        end

        def minimum_order_size(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data[1][3].to_d)
        end

        def symbol(base, quote)
          return "#{base}:#{quote}" if base.length > 3 || quote.length > 3

          "#{base}#{quote}"
        end

        def minimum_order_parameters(symbol)
          minimum = minimum_order_size(symbol)
          return minimum unless minimum.success?

          ask = current_ask_price(symbol)
          return ask unless ask.success?

          fee = fee(symbol)
          return fee unless fee.success?

          Result::Success.new(
            minimum: minimum.data,
            minimum_quote: minimum.data * ask.data,
            side: BASE,
            fee: fee.data
          )
        end

        def fee(symbol)
          Result::Success.new('0.1')
        end

        private

        def fetch_symbols
          request = @caching_client.get('/v2/conf/pub:info:pair')
          response = JSON.parse(request.body)

          Result::Success.new(response)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Bitfinex symbols", RECOVERABLE)
        end

        def fetch_symbol(symbol)
          symbols = fetch_symbols
          return symbols unless symbols.success?

          symbol_details = symbols.data[0].detect { |s| s[0] == symbol }
          Result::Success.new(symbol_details)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Bitfinex symbol", RECOVERABLE)
        end

        def fetch_ticker(symbol)
          request = @base_client.get('/v2/tickers', symbols: "t#{symbol}")
          response = JSON.parse(request.body)

          Result::Success.new(response)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Bitfinex ticker", RECOVERABLE)
        end

        def current_bid_ask_price(symbol)
          response = fetch_ticker(symbol)
          return response unless response.success?

          response = response.data
          bid = response[0][1].to_d
          ask = response[0][3].to_d

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new("Couldn't fetch bid/ask price from Bitfinex", RECOVERABLE)
        end

        def skip_symbol?(symbol_info)
          symbol_info[0][0] == 'f' # f at Bitfinex stands for funding not trading symbol
        end

        def get_quote(symbol_info)
          splited = symbol_info[0].split(':')
          return splited[1] if splited.length > 1

          splited[0][-3...]
        end

        def get_base(symbol_info)
          splited = symbol_info[0].split(':')
          return splited[0][0...] if splited.length > 1

          splited[0][0...3]
        end
      end
    end
  end
end
