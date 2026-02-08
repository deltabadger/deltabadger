require 'result'

module ExchangeApi
  module Markets
    module Bitget
      class Market < BaseMarket
        API_URL = 'https://api.bitget.com'.freeze

        def fetch_all_symbols
          conn = Faraday.new(url: API_URL) do |f|
            f.response :json
            f.adapter :net_http
          end
          response = conn.get('/api/v2/spot/public/symbols')
          data = response.body['data']
          market_symbols = data.select { |s| s['status'] == 'online' }.map do |symbol_info|
            base = symbol_info['baseCoin']
            quote = symbol_info['quoteCoin']
            MarketSymbol.new(base, quote)
          end
          Result::Success.new(market_symbols)
        rescue StandardError => e
          Result::Failure.new("Bitget exchange info is unavailable. Error: #{e}", RECOVERABLE.to_s)
        end

        def symbol(base, quote)
          "#{base}#{quote}"
        end

        def base_decimals(symbol)
          instrument = fetch_symbol(symbol)
          return instrument unless instrument.success?

          Result::Success.new(instrument.data['quantityPrecision'].to_i)
        end

        def quote_decimals(symbol)
          instrument = fetch_symbol(symbol)
          return instrument unless instrument.success?

          Result::Success.new(instrument.data['quotePrecision'].to_i)
        end

        def minimum_order_parameters(symbol)
          instrument = fetch_symbol(symbol)
          return instrument unless instrument.success?

          min_base = instrument.data['minTradeAmount'].to_f
          min_quote = (instrument.data['minTradeUSDT'] || 0).to_f

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
          response = conn.get('/api/v2/spot/public/symbols', { symbol: symbol })
          data = response.body['data']
          return Result::Failure.new("Couldn't find symbol #{symbol} on Bitget") if data.blank?

          Result::Success.new(data.first)
        rescue StandardError => e
          Result::Failure.new("Couldn't fetch symbol from Bitget. Error: #{e}", RECOVERABLE.to_s)
        end

        def current_bid_ask_price(symbol)
          conn = Faraday.new(url: API_URL) do |f|
            f.response :json
            f.adapter :net_http
          end
          response = conn.get('/api/v2/spot/market/tickers', { symbol: symbol })
          data = response.body['data']
          return Result::Failure.new("Couldn't fetch ticker for #{symbol} from Bitget") if data.blank?

          bid = data.first['bidPr'].to_f
          ask = data.first['askPr'].to_f

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError => e
          Result::Failure.new("Couldn't fetch bid/ask price from Bitget. Error: #{e}", RECOVERABLE.to_s)
        end
      end
    end
  end
end
