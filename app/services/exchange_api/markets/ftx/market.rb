require 'result'
module ExchangeApi
  module Markets
    module Ftx
      class Market < BaseMarket
        include ExchangeApi::Clients::Ftx

        BASE_URL = 'https://ftx.com/api'.freeze

        def all_symbols
          symbols_url = BASE_URL + '/markets'
          request = Faraday.get(symbols_url)

          response = JSON.parse(request.body)
          market_symbols = response.fetch('result').map do |symbol_info|
            base = get_base(symbol_info)
            quote = get_quote(symbol_info)
            MarketSymbol.new(base, quote)
          end
          Result::Success.new(market_symbols)
        rescue StandardError
          Result::Failure.new("Couldn't fetch FTX symbols", RECOVERABLE)
        end

        def minimum_order_price(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data['priceIncrement'].to_f)
        end

        def minimum_base_size(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data['sizeIncrement'].to_f)
        end

        def base_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          result = number_of_decimal_points(response.data['sizeIncrement'].to_f)

          Result::Success.new(result)
        rescue StandardError
          Result::Failure.new("Couldn't fetch FTX symbol details", RECOVERABLE)
        end

        def quote_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          result = number_of_decimal_points(response.data['priceIncrement'].to_f)

          Result::Success.new(result)
        rescue StandardError
          Result::Failure.new("Couldn't fetch FTX symbol details", RECOVERABLE)
        end

        def symbol(base, quote)
          return "#{base}-#{quote}" if future?(quote)

          "#{base}/#{quote}"
        end

        private

        def fetch_symbol(symbol)
          url = BASE_URL + "/markets/#{symbol}"
          request = Faraday.get(url)
          response = JSON.parse(request.body)

          Result::Success.new(response.fetch('result'))
        rescue StandardError
          Result::Failure.new("Couldn't fetch chosen symbol from FTX", RECOVERABLE)
        end

        def current_bid_ask_price(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          response = response.data
          bid = response['bid'].to_f
          ask = response['ask'].to_f

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new("Couldn't fetch bid/ask price from FTX", RECOVERABLE)
        end

        def get_quote(symbol_info)
          return symbol_info.fetch('name').split('-')[1] if symbol_info.fetch('type') == 'future'

          symbol_info.fetch('name').split('/')[1]
        end

        def get_base(symbol_info)
          return symbol_info.fetch('name').split('-')[0] if symbol_info.fetch('type') == 'future'

          symbol_info.fetch('name').split('/')[0]
        end

        def future?(quote)
          quote == 'PERP' || /\A\d+\z/.match(quote)
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
