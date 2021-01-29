require 'result'
module ExchangeApi
  module Markets
    module Gemini
      class Market < BaseMarket
        #include ExchangeApi::Clients::Gemini

        BASE_URL = 'https://api.gemini.com/v1'.freeze

        def initialize
          super
        end

        def all_symbols
          symbols_url = BASE_URL + '/symbols'
          request = Faraday.get(symbols_url)

          response = JSON.parse(request.body)
          market_symbols = response.map do |symbol_info|
            base = get_base(symbol_info)
            quote = get_quote(symbol_info)
            MarketSymbol.new(base, quote)
          end
          Result::Success.new(market_symbols)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Coinbase symbols", RECOVERABLE)
        end

        def minimum_base_size(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data['min_order_size'].to_f)
        end

        def base_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          result = response.data['quote_increment'].to_i

          Result::Success.new(result)
        end

        def quote_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          result = response.data['tick_size'].to_i

          Result::Success.new(result)
        end

        def symbol(base, quote)
          "#{base}#{quote}"
        end

        private

        def fetch_symbol(symbol)
          url = BASE_URL + "/symbols/details/#{symbol}"
          request = Faraday.get(url)
          response = JSON.parse(request.body)

          Result::Success.new(response)
        rescue StandardError
          Result::Failure.new("Couldn't fetch chosen symbol from Coinbase", RECOVERABLE)
        end

        def current_bid_ask_price(symbol)
          url = BASE_URL + "/pubticker/#{symbol}"
          request = Faraday.get(url)

          response = JSON.parse(request.body)

          bid = response['bid'].to_f
          ask = response['ask'].to_f

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new("Couldn't fetch bid/ask price from Coinbase", RECOVERABLE)
        end

        def get_quote(symbol)
          symbol[-3...].upcase
        end

        def get_base(symbol)
          symbol[0...-3].upcase
        end
      end
    end
  end
end
