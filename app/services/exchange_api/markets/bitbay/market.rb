require 'result'

module ExchangeApi
  module Markets
    module Bitbay
      class Market < BaseMarket
        include ExchangeApi::Clients::Bitbay

        TICKER_URL = 'https://api.bitbay.net/rest/trading/ticker'.freeze
        ALL_SYMBOLS_CACHE_KEY = 'bitbay_all_symbols'.freeze

        def minimum_order_price(symbol)
          if symbol.include? '-BTC'
            thousand_satoshis = 0.00001
            return Result::Success.new(thousand_satoshis)
          end

          response = fetch_symbol(symbol)
          return response unless response.success?

          minimum_quote_price = response.data.dig('ticker', 'market', 'second', 'minOffer')
          Result::Success.new(minimum_quote_price.to_f)
        end

        def fetch_all_symbols
          response = JSON.parse(Faraday.get(TICKER_URL).body)

          symbols_data = response['items']
          all_symbols = symbols_data.map do |_, symbol_data|
            base, quote = symbol_data.fetch('market').fetch('code').split('-')
            MarketSymbol.new(base, quote)
          end

          Result::Success.new(all_symbols)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Bitbay symbols", RECOVERABLE)
        end

        def base_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data.dig('ticker', 'market', 'first', 'scale'))
        end

        def quote_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data.dig('ticker', 'market', 'second', 'scale'))
        end

        def quote_tick_size_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data.dig('ticker', 'market', 'second', 'scale'))
        end

        def symbol(base, quote)
          "#{base}-#{quote}"
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

        def fetch_symbol(symbol)
          cache_key = symbol_cache_key(symbol)
          return Result::Success.new(Rails.cache.read(cache_key)) if Rails.cache.exist?(cache_key)

          url = "#{TICKER_URL}/#{symbol}"
          response = JSON.parse(Faraday.get(url).body)
          Rails.cache.write(cache_key, response, expires_in: 1.hour)
          Result::Success.new(response)
        rescue StandardError
          Result::Failure.new('Could not fetch chosen symbol from Bitbay', RECOVERABLE)
        end

        def current_bid_ask_price(symbol)
          url = "https://bitbay.net/API/Public/#{symbol}/ticker.json"
          response = JSON.parse(Faraday.get(url).body)

          bid = response.fetch('bid').to_f
          ask = response.fetch('ask').to_f

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new('Could not fetch current price from Bitbay', RECOVERABLE)
        end

        def symbol_cache_key(symbol)
          "bitbay_symbol_#{symbol}"
        end
      end
    end
  end
end
