require 'result'
module ExchangeApi
  module Markets
    module CoinbasePro
      class Market < BaseMarket
        include ExchangeApi::Clients::CoinbasePro

        PRODUCTS_URL = 'https://api.pro.coinbase.com/products'.freeze

        def initialize
          super
        end

        def all_symbols
          request = Faraday.get(PRODUCTS_URL)

          response = JSON.parse(request.body)
          market_symbols = response.map do |symbol_info|
            base = symbol_info['base_currency']
            quote = symbol_info['quote_currency']
            MarketSymbol.new(base, quote)
          end
          Result::Success.new(market_symbols)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Coinbase symbols", RECOVERABLE)
        end

        def minimum_order_price(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data['min_market_funds'].to_f)
        end

        def minimum_base_size(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data['base_min_size'].to_f)
        end

        def base_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          result = number_of_decimal_points(response.data['base_increment'].to_f)

          Result::Success.new(result)
        end

        def quote_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          result = number_of_decimal_points(response.data['quote_increment'].to_f)

          Result::Success.new(result)
        end

        def symbol(base, quote)
          "#{base}-#{quote}"
        end

        private

        def fetch_symbol(symbol)
          url = PRODUCTS_URL + "/#{symbol}"
          request = Faraday.get(url)
          response = JSON.parse(request.body)

          Result::Success.new(response)
        rescue StandardError
          Result::Failure.new("Couldn't fetch chosen symbol from Coinbase", RECOVERABLE)
        end

        def current_bid_ask_price(symbol)
          url = PRODUCTS_URL + "/#{symbol}/book"
          request = Faraday.get(url)

          response = JSON.parse(request.body)

          bid = response['bids'][0][0].to_f
          ask = response['asks'][0][0].to_f

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new("Couldn't fetch bid/ask price from Coinbase", RECOVERABLE)
        end

        def number_of_decimal_points(number)
          number_str = number.to_s
          return 0 unless number_str.include? '.'

          number_str.split('.')[1].length
        end
      end
    end
  end
end
