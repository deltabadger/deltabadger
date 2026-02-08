require 'result'

module ExchangeApi
  module Markets
    module Kucoin
      class Market < BaseMarket
        API_URL = 'https://api.kucoin.com'.freeze

        def fetch_all_symbols
          conn = Faraday.new(url: API_URL) do |f|
            f.response :json
            f.adapter :net_http
          end
          response = conn.get('/api/v2/symbols')
          data = response.body['data']
          market_symbols = data.select { |s| s['enableTrading'] }.map do |symbol_info|
            base = symbol_info['baseCurrency']
            quote = symbol_info['quoteCurrency']
            MarketSymbol.new(base, quote)
          end
          Result::Success.new(market_symbols)
        rescue StandardError => e
          Result::Failure.new("KuCoin exchange info is unavailable. Error: #{e}", RECOVERABLE.to_s)
        end

        def symbol(base, quote)
          "#{base}-#{quote}"
        end

        def base_decimals(symbol)
          instrument = fetch_symbol(symbol)
          return instrument unless instrument.success?

          increment = instrument.data['baseIncrement']
          Result::Success.new(Utilities::Number.decimals(increment))
        end

        def quote_decimals(symbol)
          instrument = fetch_symbol(symbol)
          return instrument unless instrument.success?

          increment = instrument.data['quoteIncrement']
          Result::Success.new(Utilities::Number.decimals(increment))
        end

        def minimum_order_parameters(symbol)
          instrument = fetch_symbol(symbol)
          return instrument unless instrument.success?

          min_base = instrument.data['baseMinSize'].to_f
          min_quote = instrument.data['quoteMinSize'].to_f

          Result::Success.new(
            minimum: min_base,
            minimum_quote: min_quote,
            side: BASE
          )
        end

        private

        def fetch_symbol(symbol)
          conn = Faraday.new(url: API_URL) do |f|
            f.response :json
            f.adapter :net_http
          end
          response = conn.get('/api/v2/symbols')
          data = response.body['data']
          found = data.find { |s| s['symbol'] == symbol }
          return Result::Failure.new("Couldn't find symbol #{symbol} on KuCoin") if found.nil?

          Result::Success.new(found)
        rescue StandardError => e
          Result::Failure.new("Couldn't fetch symbol from KuCoin. Error: #{e}", RECOVERABLE.to_s)
        end

        def current_bid_ask_price(symbol)
          conn = Faraday.new(url: API_URL) do |f|
            f.response :json
            f.adapter :net_http
          end
          response = conn.get('/api/v1/market/orderbook/level2_20', { symbol: symbol })
          data = response.body['data']
          bid = data['bids'][0][0].to_f
          ask = data['asks'][0][0].to_f

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError => e
          Result::Failure.new("Couldn't fetch bid/ask price from KuCoin. Error: #{e}", RECOVERABLE.to_s)
        end
      end
    end
  end
end
