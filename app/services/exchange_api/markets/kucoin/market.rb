require 'result'

module ExchangeApi
  module Markets
    module Kucoin
      class Market < BaseMarket
        include ExchangeApi::Clients::Kucoin

        def initialize
          @base_client = base_client(API_URL)
          @caching_client = caching_client(API_URL)
        end

        def fetch_all_symbols
          response = fetch_symbols
          return response unless response.success?

          market_symbols = response.data.fetch('data').map do |symbol_info|
            base = get_base(symbol_info)
            quote = get_quote(symbol_info)

            MarketSymbol.new(base, quote)
          end
          Result::Success.new(market_symbols)
        rescue StandardError
          Result::Failure.new("Couldn't fetch KuCoin symbols", **RECOVERABLE)
        end

        def minimum_quote_size(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data['quoteMinSize'].to_d)
        end

        def minimum_base_size(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data['baseMinSize'].to_d)
        end

        def quote_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          result = GetNumberOfDecimalPoints.call(response.data['quoteIncrement'])
          Result::Success.new(result)
        rescue StandardError
          Result::Failure.new("Couldn't fetch KuCoin symbol details", **RECOVERABLE)
        end

        def base_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          result = GetNumberOfDecimalPoints.call(response.data['baseIncrement'])

          Result::Success.new(result)
        rescue StandardError
          Result::Failure.new("Couldn't fetch KuCoin symbol details", **RECOVERABLE)
        end

        def price_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          result = GetNumberOfDecimalPoints.call(response.data['priceIncrement'])
          Result::Success.new(result)
        rescue StandardError
          Result::Failure.new("Couldn't fetch KuCoin symbol details", **RECOVERABLE)
        end

        def symbol(base, quote)
          "#{base.upcase}-#{quote.upcase}"
        end

        def minimum_order_parameters(symbol)
          minimum = minimum_quote_size(symbol)
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

        def current_fee
          exchange_id = Exchange.find_by(name: 'KuCoin').id
          fee_api_keys = FeeApiKey.find_by(exchange_id: exchange_id)
          path = '/api/v1/base-fee'.freeze
          response = @caching_client.get(path, nil, headers(fee_api_keys.key, fee_api_keys.secret, fee_api_keys.passphrase, '', path, 'GET')).body
          JSON.parse(response)['data']['makerFeeRate'].to_f * 100
        end

        private

        def fetch_symbols
          request = @caching_client.get('/api/v1/symbols')
          response = JSON.parse(request.body)

          Result::Success.new(response)
        rescue StandardError
          Result::Failure.new("Couldn't fetch KuCoin symbols", **RECOVERABLE)
        end

        def fetch_symbol(symbol)
          symbols = fetch_symbols
          return symbols unless symbols.success?

          symbol_details = symbols.data.fetch('data').detect { |s| s.fetch('symbol') == symbol }
          Result::Success.new(symbol_details)
        rescue StandardError
          Result::Failure.new("Couldn't fetch KuCoin symbol", **RECOVERABLE)
        end

        def fetch_ticker(symbol)
          request = @base_client.get('/api/v1/market/orderbook/level1', symbol: symbol)
          response = JSON.parse(request.body)

          Result::Success.new(response['data'])
        rescue StandardError
          Result::Failure.new("Couldn't fetch KuCoin ticker", **RECOVERABLE)
        end

        def current_bid_ask_price(symbol)
          response = fetch_ticker(symbol)
          return response unless response.success?

          response = response.data
          bid = response['bestBid'].to_d
          ask = response['bestAsk'].to_d

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new("Couldn't fetch bid/ask price from KuCoin", **RECOVERABLE)
        end

        def get_quote(symbol_info)
          symbol_info.fetch('symbol').split('-')[1]
        end

        def get_base(symbol_info)
          symbol_info.fetch('symbol').split('-')[0]
        end
      end
    end
  end
end
