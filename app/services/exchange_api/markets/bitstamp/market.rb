require 'result'

module ExchangeApi
  module Markets
    module Bitstamp
      class Market < BaseMarket
        include ExchangeApi::Clients::Bitstamp
        MIN_PRICE_MULTIPLIER = 1.005

        def initialize
          @base_client = base_client(API_URL)
          @caching_client = caching_client(API_URL)
        end

        def fetch_all_symbols
          response = fetch_symbols
          return response unless response.success?

          market_symbols = response.data.reject { |s| disabled_symbol?(s) }.map do |symbol_info|
            base = get_base(symbol_info)
            quote = get_quote(symbol_info)

            MarketSymbol.new(base, quote)
          end
          Result::Success.new(market_symbols)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Bitstamp symbols", RECOVERABLE)
        end

        def minimum_order_price(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(get_minimum_order_value(response.data))
        end

        def minimum_base_size(symbol)
          min_price = minimum_order_price(symbol)
          return min_price unless min_price.success?

          # multiply min_price to ensure that base will exceed minimums
          min_price = min_price.data * MIN_PRICE_MULTIPLIER
          bid = current_bid_price(symbol)
          return bid unless bid.success?

          volume_decimals = base_decimals(symbol)
          return volume_decimals unless volume_decimals.success?

          Result::Success.new((min_price / bid.data).ceil(volume_decimals.data))
        end

        def quote_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          result = response.data['counter_decimals']

          Result::Success.new(result)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Bitstamp symbol details", RECOVERABLE)
        end

        def base_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          result = response.data['base_decimals']

          Result::Success.new(result)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Bitstamp symbol details", RECOVERABLE)
        end

        def symbol(base, quote)
          "#{base.downcase}#{quote.downcase}"
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
          exchange_id = Exchange.find_by(name: 'Bitstamp').id
          fee_api_keys = FeeApiKey.find_by(exchange_id: exchange_id)
          path = '/api/v2/balance/'
          response = @caching_client.post(path, nil, headers(fee_api_keys.key, fee_api_keys.secret, nil, path, 'POST', '')).body
          JSON.parse(response)['btcusd_fee']
        end

        private

        def fetch_symbols
          request = @caching_client.get('/api/v2/trading-pairs-info/')

          response = JSON.parse(request.body)

          Result::Success.new(response)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Bitstamp symbols", RECOVERABLE)
        end

        def fetch_symbol(symbol)
          books = fetch_symbols
          return books unless books.success?

          symbol_details = books.data.detect { |b| b.fetch('url_symbol') == symbol }
          Result::Success.new(symbol_details)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Bitstamp symbol", RECOVERABLE)
        end

        def fetch_ticker(symbol)
          request = @base_client.get("/api/v2/ticker/#{symbol}")
          response = JSON.parse(request.body)

          Result::Success.new(response)
        rescue StandardError
          Result::Failure.new("Couldn't fetch chosen symbol from Bitstamp", RECOVERABLE)
        end

        def current_bid_ask_price(symbol)
          response = fetch_ticker(symbol)
          return response unless response.success?

          response = response.data
          bid = response['bid'].to_d
          ask = response['ask'].to_d

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new("Couldn't fetch bid/ask price from Bitstamp", RECOVERABLE)
        end

        def disabled_symbol?(symbol_info)
          symbol_info.fetch('trading') == 'Disabled'
        end

        def get_quote(symbol_info)
          symbol_info.fetch('name').split('/')[1].upcase
        end

        def get_base(symbol_info)
          symbol_info.fetch('name').split('/')[0].upcase
        end

        def get_minimum_order_value(response)
          response.fetch('minimum_order').split(' ')[0].to_d
        end
      end
    end
  end
end
