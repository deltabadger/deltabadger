require 'result'
module ExchangeApi
  module Markets
    module Gemini
      class Market < BaseMarket
        include ExchangeApi::Clients::Gemini

        API_URL = 'https://api.gemini.com'.freeze

        def initialize
          @base_client = base_client(API_URL)
          @caching_client = caching_client(API_URL)
        end

        def fetch_all_symbols
          request = @caching_client.get('/v1/symbols')
          response = JSON.parse(request.body)
          market_symbols = response.map do |symbol|
            base = get_base(symbol)
            quote = get_quote(symbol)
            raise StandardError if base.nil? || quote.nil?

            MarketSymbol.new(base, quote)
          end
          Result::Success.new(market_symbols)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Gemini symbols", RECOVERABLE.to_s)
        end

        def minimum_base_size(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data['min_order_size'].to_f)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Gemini minimums", RECOVERABLE.to_s)
        end

        def base_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          result = GetNumberOfDecimalPoints.call(response.data['tick_size'])

          Result::Success.new(result)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Gemini symbol details", RECOVERABLE.to_s)
        end

        def quote_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          result = GetNumberOfDecimalPoints.call(response.data['quote_increment'])

          Result::Success.new(result)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Gemini symbol details", RECOVERABLE.to_s)
        end

        def symbol(base, quote)
          "#{base}#{quote}"
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
          exchange_id = Exchange.find_by(name: 'Gemini').id
          fee_api_keys = FeeApiKey.find_by(exchange_id: exchange_id)
          path = '/v1/notionalvolume'.freeze
          url = API_URL + path
          request_params = {
            request: path,
            nonce: Time.now.strftime('%s%L')
          }
          body = request_params.to_json
          response = Faraday.post(url, body, headers(fee_api_keys.key, fee_api_keys.secret, body)).body
          JSON.parse(response)['api_maker_fee_bps'].to_f / 100
        end

        private

        def fetch_symbol(symbol)
          request = @caching_client.get("/v1/symbols/details/#{symbol}")
          response = JSON.parse(request.body)
          return Result::Failure.new("#{symbol} pair is no longer available on Gemini") if response['status'] == 'closed'

          Result::Success.new(response)
        rescue StandardError
          Result::Failure.new("Couldn't fetch chosen symbol from Gemini", RECOVERABLE.to_s)
        end

        def current_bid_ask_price(symbol)
          request = @base_client.get("/v1/pubticker/#{symbol}")
          response = JSON.parse(request.body)

          bid = response['bid'].to_f
          ask = response['ask'].to_f

          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new("Couldn't fetch bid/ask price from Gemini", RECOVERABLE.to_s)
        end

        def get_quote(symbol)
          guess_quote(symbol) || fetch_quote(symbol)
        end

        def guess_quote(symbol)
          return 'USD' if symbol == 'paxgusd'

          known_four_char_quotes = %w[gusd usdt]
          known_three_char_quotes = %w[usd eur gbp sgd dai btc eth ltc bch fil]

          if known_four_char_quotes.include?(symbol[-4..])
            symbol[-4..].upcase
          elsif known_three_char_quotes.include?(symbol[-3..])
            symbol[-3..].upcase
          end
        end

        def fetch_quote(symbol)
          symbol_details = fetch_symbol(symbol)
          return nil unless symbol_details.success?

          symbol_details.data['quote_currency']
        end

        def get_base(symbol)
          guess_base(symbol) || fetch_base(symbol)
        end

        def guess_base(symbol)
          quote = guess_quote(symbol)
          return nil if quote.nil?

          symbol[0...-quote.length].upcase
        end

        def fetch_base(symbol)
          symbol_details = fetch_symbol(symbol)
          return nil unless symbol_details.success?

          symbol_details.data['base_currency']
        end
      end
    end
  end
end
