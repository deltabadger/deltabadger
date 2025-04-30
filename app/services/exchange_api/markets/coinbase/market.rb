require 'result'

module ExchangeApi
  module Markets
    module Coinbase
      class Market < BaseMarket
        include ExchangeApi::Clients::Coinbase

        API_URL = 'https://api.coinbase.com'.freeze

        # private
        attr_reader :exchange_id
        attr_reader :fee_api_keys

        def initialize
          super
          @base_client = base_client(API_URL)
          @caching_client = caching_client(API_URL)
          @exchange_id ||= Exchange.find_by(name: 'Coinbase').id
          @fee_api_keys ||= FeeApiKey.find_by(exchange_id: exchange_id)
        end

        def fetch_all_symbols
          response = Rails.cache.fetch('coinbase_symbols', expires_in: 5.minutes) do
            request = unauthenticated_request('/api/v3/brokerage/market/products')
            JSON.parse(request.body)
          end
          market_symbols = response['products'].map do |symbol_info|
            base = symbol_info['base_currency_id']
            quote = symbol_info['quote_currency_id']
            MarketSymbol.new(base, quote)
          end
          Result::Success.new(market_symbols)
        rescue StandardError => e
          Result::Failure.new("Couldn't fetch Coinbase symbols. Error #{e}", **RECOVERABLE)
        end

        def minimum_order_price(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data['quote_min_size'].to_f)
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
          response = @caching_client.get('/api/v3/brokerage/transaction_summary')
          JSON.parse(response.body)['fee_tier'].maker_fee_rate.to_f * 100
        end

        def fetch_symbol(symbol)
          response = Rails.cache.fetch("coinbase_symbol_#{symbol}", expires_in: 5.minutes) do
            request = authenticated_request("/api/v3/brokerage/market/products/#{symbol}")
            JSON.parse(request.body)
          end
          Result::Success.new(response)
        rescue StandardError
          Result::Failure.new("Couldn't fetch chosen symbol from Coinbase", **RECOVERABLE)
        end

        def current_bid_ask_price(symbol)
          response = Rails.cache.fetch("coinbase_bid_ask_price_#{symbol}", expires_in: 1.seconds) do
            request = authenticated_request('/api/v3/brokerage/best_bid_ask')
            JSON.parse(request.body)
          end
          bid_price = nil
          ask_price = nil
          response['pricebooks'].each do |pricebook|
            next unless pricebook['product_id'] == symbol

            bid_price = pricebook['bids'][0]['price'].to_f
            ask_price = pricebook['asks'][0]['price'].to_f
            break
          end
          Result::Success.new(BidAskPrice.new(bid_price, ask_price))
        rescue StandardError => e
          Result::Failure.new("Couldn't fetch bid/ask price from Coinbase. Error: #{e}", **RECOVERABLE)
        end

        private

        def unauthenticated_request(path)
          url = API_URL + path
          @base_client.get(url, nil, headers('', '', '', url, 'GET'))
        end

        def authenticated_request(path)
          url = API_URL + path
          @base_client.get(url, nil, headers(fee_api_keys.key, fee_api_keys.secret, '', url, 'GET'))
        end
      end
    end
  end
end
