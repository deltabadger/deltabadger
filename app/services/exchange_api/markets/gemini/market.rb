require 'result'

module ExchangeApi
  module Markets
    module Gemini
      class Market < BaseMarket
        API_URL = 'https://api.gemini.com'.freeze

        def fetch_all_symbols
          conn = Faraday.new(url: API_URL) do |f|
            f.response :json
            f.adapter :net_http
          end
          response = conn.get('/v1/symbols')
          symbols = response.body
          market_symbols = symbols.map do |symbol|
            detail_response = conn.get("/v1/symbols/details/#{symbol}")
            detail = detail_response.body
            base = detail['base_currency'].upcase
            quote = detail['quote_currency'].upcase
            MarketSymbol.new(base, quote)
          end
          Result::Success.new(market_symbols)
        rescue StandardError => e
          Result::Failure.new("Gemini exchange info is unavailable. Error: #{e}", RECOVERABLE.to_s)
        end

        def symbol(base, quote)
          "#{base}#{quote}".downcase
        end

        def base_decimals(symbol)
          instrument = fetch_symbol(symbol)
          return instrument unless instrument.success?

          tick_size = instrument.data['tick_size']&.to_d || '0.01'.to_d
          Result::Success.new(Utilities::Number.decimals(tick_size.to_s))
        end

        def quote_decimals(symbol)
          instrument = fetch_symbol(symbol)
          return instrument unless instrument.success?

          quote_increment = instrument.data['quote_increment']&.to_d || '0.01'.to_d
          Result::Success.new(Utilities::Number.decimals(quote_increment.to_s))
        end

        def minimum_order_parameters(symbol)
          instrument = fetch_symbol(symbol)
          return instrument unless instrument.success?

          min_size = instrument.data['min_order_size'].to_f
          ask = current_ask_price(symbol)
          return ask unless ask.success?

          Result::Success.new(
            minimum: min_size,
            minimum_quote: min_size * ask.data,
            side: BASE
          )
        end

        private

        def fetch_symbol(symbol)
          conn = Faraday.new(url: API_URL) do |f|
            f.response :json
            f.adapter :net_http
          end
          response = conn.get("/v1/symbols/details/#{symbol}")
          data = response.body
          return Result::Failure.new("Couldn't find symbol #{symbol} on Gemini") if data.nil? || data['status'] == 'error'

          Result::Success.new(data)
        rescue StandardError => e
          Result::Failure.new("Couldn't fetch symbol from Gemini. Error: #{e}", RECOVERABLE.to_s)
        end

        def current_bid_ask_price(symbol)
          conn = Faraday.new(url: API_URL) do |f|
            f.response :json
            f.adapter :net_http
          end
          response = conn.get("/v2/ticker/#{symbol}")
          data = response.body
          bid = data['bid'].to_f
          ask = data['ask'].to_f

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError => e
          Result::Failure.new("Couldn't fetch bid/ask price from Gemini. Error: #{e}", RECOVERABLE.to_s)
        end
      end
    end
  end
end
