require 'result'

module ExchangeApi
  module Markets
    module Bitso
      class Market < BaseMarket
        include ExchangeApi::Clients::Bitso

        def initialize
          @base_client = base_client(API_URL)
          @caching_client = caching_client(API_URL)
        end

        def fetch_all_symbols
          response = fetch_books
          return response unless response.success?

          market_symbols = response.data.fetch('payload').map do |symbol_info|
            base = get_base(symbol_info)
            quote = get_quote(symbol_info)

            MarketSymbol.new(base, quote)
          end
          Result::Success.new(market_symbols)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Bitso symbols", RECOVERABLE.to_s)
        end

        def minimum_order_price(symbol)
          response = fetch_book(symbol)
          return response unless response.success?

          Result::Success.new(response.data['minimum_value'].to_d)
        end

        def minimum_base_size(symbol)
          response = fetch_book(symbol)
          return response unless response.success?

          Result::Success.new(response.data['minimum_amount'].to_d)
        end

        def quote_decimals(symbol)
          response = fetch_book(symbol)
          return response unless response.success?

          result = GetNumberOfDecimalPoints.call(response.data['tick_size'])

          Result::Success.new(result)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Bitso symbol details", RECOVERABLE.to_s)
        end

        def symbol(base, quote)
          "#{base.downcase}_#{quote.downcase}"
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

        def current_fee
          symbol_info = fetch_book(symbol('BTC', 'USD'))
          raise StandardError unless symbol_info.success?

          symbol_info.data['fees']['flat_rate']['maker']
        end

        private

        def fetch_books
          request = @caching_client.get('/v3/available_books/')
          response = JSON.parse(request.body)
          return Result::Failure.new("Couldn't fetch Bitso books", RECOVERABLE.to_s) unless response.fetch('success', false)

          Result::Success.new(response)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Bitso books", RECOVERABLE.to_s)
        end

        def fetch_book(symbol)
          books = fetch_books
          return books unless books.success?

          symbol_details = books.data.fetch('payload').detect { |b| b.fetch('book') == symbol }
          Result::Success.new(symbol_details)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Bitso book", RECOVERABLE.to_s)
        end

        def fetch_symbol(symbol)
          request = @base_client.get('/v3/ticker/', 'book': symbol)
          response = JSON.parse(request.body)

          Result::Success.new(response.fetch('payload'))
        rescue StandardError
          Result::Failure.new("Couldn't fetch chosen symbol from Bitso", RECOVERABLE.to_s)
        end

        def current_bid_ask_price(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          response = response.data
          bid = response['bid'].to_d
          ask = response['ask'].to_d

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new("Couldn't fetch bid/ask price from Bitso", RECOVERABLE.to_s)
        end

        def get_quote(symbol_info)
          symbol_info.fetch('book').split('_')[1].upcase
        end

        def get_base(symbol_info)
          symbol_info.fetch('book').split('_')[0].upcase
        end
      end
    end
  end
end
