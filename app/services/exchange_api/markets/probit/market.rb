require 'result'

module ExchangeApi
  module Markets
    module Probit
      class Market < BaseMarket
        include ExchangeApi::Clients::Probit

        def initialize
          @base_client = base_client(API_URL)
          @caching_client = caching_client(API_URL)
        end

        def fetch_all_symbols
          request = @caching_client.get('/api/exchange/v1/market')
          response = JSON.parse(request.body)
          market_symbols = response.fetch('data').map do |symbol_info|
            base = get_base(symbol_info)
            quote = get_quote(symbol_info)

            MarketSymbol.new(base, quote)
          end
          Result::Success.new(market_symbols)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Probit symbols", RECOVERABLE)
        end

        def fetch_book(symbol)
          symbols = fetch_all_books.data['data']
          book_data = symbols.find { |s| s['id'] == symbol }
          Result::Success.new(book_data)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Probit books", RECOVERABLE)
        end

        def fetch_all_books
          request = @caching_client.get('/api/exchange/v1/market')
          response = JSON.parse(request.body)
          return Result::Failure.new("Couldn't fetch Probit books", RECOVERABLE) unless request.status == 200

          Result::Success.new(response)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Probit books", RECOVERABLE)
        end

        def base_decimals(symbol)
          response = fetch_book(symbol)
          return response unless response.success?

          Result::Success.new(response.data['quantity_precision'].to_d)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Probit symbol details", RECOVERABLE)
        end

        def quote_decimals(symbol)
          response = fetch_book(symbol)
          return response unless response.success?

          Result::Success.new(response.data['cost_precision'].to_d)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Probit symbol details", RECOVERABLE)
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

        def minimum_order_price(symbol)
          response = fetch_book(symbol)
          return response unless response.success?

          Result::Success.new(response.data['min_cost'].to_d)
        end

        def minimum_order_quantity(symbol)
          response = fetch_book(symbol)
          return response unless response.success?

          Result::Success.new(response.data['min_quantity'].to_d)
        end

        def symbol(base, quote)
          "#{base.upcase}-#{quote.upcase}"
        end

        def current_bid_ask_price(symbol)
          request = @base_client.get("/api/exchange/v1/order_book?market_id=#{symbol}", nil, nil)
          response = JSON.parse(request.body)
          first_buy = true
          first_sell = true
          lowest_bid = nil
          highest_ask = nil
          response['data'].each do |x|
            if x['side'] == 'buy'
              if first_buy
                highest_ask = x['price']
                first_buy = false
              else
                highest_ask = x['price'] if highest_ask.to_f < x['price'].to_f
              end
            elsif first_sell
              lowest_bid = x['price']
              first_sell = false
            else
              lowest_bid = x['price'] if lowest_bid.to_f > x['price'].to_f
            end
          end

          Result::Success.new(BidAskPrice.new(lowest_bid.to_f, highest_ask.to_f))
        rescue StandardError
          Result::Failure.new('Could not fetch current price from Probit', RECOVERABLE)
        end

        private

        def get_quote(symbol_info)
          symbol_info.fetch('quote_currency_id')
        end

        def get_base(symbol_info)
          symbol_info.fetch('base_currency_id')
        end
      end
    end
  end
end

