require 'result'
module ExchangeApi
  module Markets
    module CoinbasePro
      class Market < BaseMarket
        include ExchangeApi::Clients::CoinbasePro

        API_URL = 'https://api.pro.coinbase.com'.freeze

        def initialize
          super
          @base_client = base_client(API_URL)
          @caching_client = caching_client(API_URL)
        end

        def fetch_all_symbols
          request = @caching_client.get('/products')

          response = JSON.parse(request.body)
          market_symbols = response.map do |symbol_info|
            base = symbol_info['base_currency']
            quote = symbol_info['quote_currency']
            MarketSymbol.new(base, quote)
          end
          Result::Success.new(market_symbols)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Coinbase symbols", **RECOVERABLE)
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

          result = GetNumberOfDecimalPoints.call(response.data['base_increment'].to_f)

          Result::Success.new(result)
        end

        def quote_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          result = GetNumberOfDecimalPoints.call(response.data['quote_increment'].to_f)

          Result::Success.new(result)
        end

        def symbol(base, quote)
          "#{base}-#{quote}"
        end

        def minimum_order_parameters(symbol)
          minimum = minimum_order_price(symbol)
          return minimum unless minimum.success?

          minimum_limit = minimum_base_size(symbol)
          return minimum_limit unless minimum_limit.success?

          Result::Success.new(
            minimum: minimum.data,
            minimum_limit: minimum_limit.data,
            minimum_quote: minimum.data,
            side: QUOTE
          )
        end

        def limit_only?(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data['limit_only'])
        end

        def current_fee
          exchange_id = Exchange.find_by(name: 'Coinbase Pro').id
          fee_api_keys = FeeApiKey.find_by(exchange_id: exchange_id)
          path = '/fees'.freeze
          url = API_URL + path
          response = @caching_client.get(url, nil, headers(fee_api_keys.key, fee_api_keys.secret, fee_api_keys.passphrase, '', path, 'GET')).body
          JSON.parse(response)['maker_fee_rate'].to_f * 100
        end

        private

        def fetch_symbol(symbol)
          request = @caching_client.get("/products/#{symbol}")
          response = JSON.parse(request.body)

          Result::Success.new(response)
        rescue StandardError
          Result::Failure.new("Couldn't fetch chosen symbol from Coinbase", **RECOVERABLE)
        end

        def current_bid_ask_price(symbol)
          request = @base_client.get("/products/#{symbol}/book")
          response = JSON.parse(request.body)
          bid = response['bids'][0][0].to_f
          ask = response['asks'][0][0].to_f

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new("Couldn't fetch bid/ask price from Coinbase", **RECOVERABLE)
        end
      end
    end
  end
end
