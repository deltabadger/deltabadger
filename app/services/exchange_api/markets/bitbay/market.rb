require 'result'

module ExchangeApi
  module Markets
    module Bitbay
      class Market < BaseMarket
        include ExchangeApi::Clients::Bitbay

        API_URL = 'https://api.bitbay.net'.freeze

        def initialize
          @base_client = base_client(API_URL)
          @caching_client = caching_client(API_URL)
        end

        def minimum_order_price(symbol, for_base = false)
          if symbol.include? '-BTC'
            thousand_satoshis = 0.00001
            return Result::Success.new(thousand_satoshis)
          end

          response = fetch_symbol(symbol)
          return response unless response.success?

          minimum_quote_price = response.data.dig('ticker', 'market', for_base ? 'first' : 'second', 'minOffer')
          Result::Success.new(minimum_quote_price.to_f)
        end

        def fetch_all_symbols
          response = JSON.parse(@base_client.get('rest/trading/ticker').body)

          symbols_data = response['items']
          all_symbols = symbols_data.map do |_, symbol_data|
            base, quote = symbol_data.fetch('market').fetch('code').split('-')
            MarketSymbol.new(base, quote)
          end

          Result::Success.new(all_symbols)
        rescue StandardError
          Result::Failure.new("Couldn't fetch Bitbay symbols", RECOVERABLE)
        end

        def base_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data.dig('ticker', 'market', 'first', 'scale'))
        end

        def quote_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data.dig('ticker', 'market', 'second', 'scale'))
        end

        def quote_tick_size_decimals(symbol)
          response = fetch_symbol(symbol)
          return response unless response.success?

          Result::Success.new(response.data.dig('ticker', 'market', 'second', 'scale'))
        end

        def symbol(base, quote)
          "#{base}-#{quote}"
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
          exchange_id = Exchange.find_by(name: 'BitBay').id
          fee_api_keys = FeeApiKey.find_by(exchange_id: exchange_id)
          path = "/rest/trading/config/#{symbol('BTC', 'PLN')}"
          response = JSON.parse(@caching_client.get(path, nil, headers(fee_api_keys.key, fee_api_keys.secret, nil)).body)
          response['config']['buy']['commissions']['maker'].to_f * 100
        end

        private

        def fetch_symbol(symbol)
          response = JSON.parse(@caching_client.get("/rest/trading/ticker/#{symbol}").body)

          Result::Success.new(response)
        rescue StandardError
          Result::Failure.new('Could not fetch chosen symbol from Bitbay', RECOVERABLE)
        end

        def current_bid_ask_price(symbol)
          url = "https://bitbay.net/API/Public/#{symbol}/ticker.json"
          response = JSON.parse(@base_client.get(url).body)

          bid = response.fetch('bid').to_f
          ask = response.fetch('ask').to_f
          Result::Success.new(BidAskPrice.new(bid, ask))
        rescue StandardError
          Result::Failure.new('Could not fetch current price from Bitbay', RECOVERABLE)
        end
      end
    end
  end
end
