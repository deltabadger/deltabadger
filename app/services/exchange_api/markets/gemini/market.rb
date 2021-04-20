require 'result'
module ExchangeApi
  module Markets
    module Gemini
      class Market < BaseMarket
        include ExchangeApi::Clients::Gemini

        BASE_URL = 'https://api.gemini.com/v1'.freeze

        def fetch_all_symbols
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
          Result::Failure.new("Couldn't fetch Gemini symbols", RECOVERABLE)
        end

        def minimum_base_size(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data['min_order_size'].to_f)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Gemini minimums", RECOVERABLE)
        end

        def base_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          result = GetNumberOfDecimalPoints.call(response.data['tick_size'])

          Result::Success.new(result)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Gemini symbol details", RECOVERABLE)
        end

        def quote_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          result = response.data['tick_size'].to_i

          Result::Success.new(result)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Gemini symbol details", RECOVERABLE)
        end

        def symbol(base, quote)
          "#{base}#{quote}"
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

        def fetch_symbol(symbol)
          url = BASE_URL + "/symbols/details/#{symbol}"
          request = Faraday.get(url)
          response = JSON.parse(request.body)

          Result::Success.new(response)
        rescue StandardError
          Result::Failure.new("Couldn't fetch chosen symbol from Gemini", RECOVERABLE)
        end

        def current_bid_ask_price(symbol)
          url = BASE_URL + "/pubticker/#{symbol}"
          request = Faraday.get(url)

          response = JSON.parse(request.body)

          bid = response['bid'].to_f
          ask = response['ask'].to_f

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new("Couldn't fetch bid/ask price from Gemini", RECOVERABLE)
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
