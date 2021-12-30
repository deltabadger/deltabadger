require 'result'

module ExchangeApi
  module Markets
    module Ftx
      class Market < BaseMarket
        include ExchangeApi::Clients::Ftx

        def initialize(url_base:)
          @base_client = base_client(url_base)
          @caching_client = caching_client(url_base)
        end

        def fetch_all_symbols
          request = @caching_client.get('/api/markets')
          response = JSON.parse(request.body)
          market_symbols = response.fetch('result').map do |symbol_info|
            base = get_base(symbol_info)
            quote = get_quote(symbol_info)
            next if base.blank? || quote.blank?

            MarketSymbol.new(base, quote)
          end
          Result::Success.new(market_symbols.compact)
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

          result = GetNumberOfDecimalPoints.call(response.data['sizeIncrement'])

          Result::Success.new(result)
        rescue StandardError
          Result::Failure.new("Couldn't fetch FTX symbol details", RECOVERABLE)
        end

        def quote_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          result = GetNumberOfDecimalPoints.call(response.data['priceIncrement'])

          Result::Success.new(result)
        rescue StandardError
          Result::Failure.new("Couldn't fetch FTX symbol details", RECOVERABLE)
        end

        def symbol(base, quote)
          return base if future?(base)

          "#{base}/#{quote}"
        end

        def minimum_order_parameters(symbol)
          minimum = minimum_base_size(symbol)
          return minimum unless minimum.success?

          ask = current_ask_price(symbol)
          return ask unless ask.success?

          Result::Success.new(
            minimum: minimum.data,
            minimum_quote: minimum.data * ask.data,
            side: BASE
          )
        end

        def current_fee
          url_base = @base_client.url_prefix.to_s
          exchange_id = Exchange.find_by(name: url_base.include?('us') ? 'FTX.US' : 'FTX').id
          fee_api_keys = FeeApiKey.find_by(exchange_id: exchange_id)
          path = '/api/account'.freeze
          url = url_base[0...-1] + path
          headers = get_headers(url, fee_api_keys.key, fee_api_keys.secret, '', path, 'GET', false, nil)
          response = Faraday.get(url, nil, headers).body
          JSON.parse(response)['result']['makerFee'].to_f * 100
        end

        def subaccounts(api_keys)
          url_base = @base_client.url_prefix.to_s
          path = '/api/subaccounts'.freeze
          url = url_base[0...-1] + path
          headers = get_headers(url, api_keys.key, api_keys.secret, '', path, 'GET', false, nil)
          response = Faraday.get(url, nil, headers).body
          return Result::Failure.new(["Couldn't fetch subaccounts from FTX", RECOVERABLE]) unless JSON.parse(response)['success']

          Result::Success.new(JSON.parse(response)['result'].map { |x| x['nickname'] })
        rescue StandardError
          Result::Failure.new(["Couldn't fetch subaccounts from FTX", RECOVERABLE])
        end

        private

        def fetch_symbol(symbol, cached = true)
          client = get_client(cached)
          request = client.get("/api/markets/#{symbol}")
          response = JSON.parse(request.body)
          Result::Success.new(response.fetch('result'))
        rescue StandardError
          Result::Failure.new("Couldn't fetch chosen symbol from FTX", RECOVERABLE)
        end

        def get_client(cached = true)
          cached ? @caching_client : @base_client
        end

        def current_bid_ask_price(symbol)
          response = fetch_symbol(symbol, false)
          return response unless response.success?

          response = response.data
          bid = response['bid'].to_f
          ask = response['ask'].to_f

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new("Couldn't fetch bid/ask price from FTX", RECOVERABLE)
        end

        def get_quote(symbol_info)
          return 'USD' if symbol_info.fetch('type') == 'future'

          symbol_info.fetch('name').split('/')[1]
        end

        def get_base(symbol_info)
          return symbol_info.fetch('name') if symbol_info.fetch('type') == 'future'

          symbol_info.fetch('name').split('/')[0]
        end

        def future?(base)
          base.include?('-')
        end
      end
    end
  end
end
