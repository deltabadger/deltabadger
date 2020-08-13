module ExchangeApi
  module Clients
    class Binance < ExchangeApi::Clients::Base
      URL_BASE = 'https://api.binance.com/api/v3'.freeze
      ORDER_DOES_NOT_EXIST = -2011
      MIN_TRANSACTION_PRICES = {
        BKRW: 2000,
        IDRT: 40_000,
        NGN: 1000,
        RUB: 200,
        ZAR: 200,
        UAH: 200
      }.freeze
      DEFAULT_MIN_TRANSACTION_PRICE = 20

      def initialize(api_key:, api_secret:, map_errors: ExchangeApi::MapErrors::Binance.new)
        @api_key = api_key
        @api_secret = api_secret
        @map_errors = map_errors
      end

      def validate_credentials
        request = signed_client.delete('order', symbol: 'ETHBTC', orderId: '9' * 10)
        response = JSON.parse(request.body)
        response['code'] == ORDER_DOES_NOT_EXIST
      rescue StandardError
        false
      end

      def current_bid_ask_price(currency)
        symbol = "BTC#{currency.upcase}"
        request = unsigned_client.get('ticker/bookTicker', { symbol: symbol }, {})
        response = JSON.parse(request.body)

        bid = response.fetch('bidPrice').to_f
        ask = response.fetch('askPrice').to_f

        Result::Success.new(BidAskPrice.new(bid, ask))
      rescue StandardError
        Result::Failure.new('Could not fetch current price from Binance')
      end

      def buy(currency:, price:)
        make_order('BUY', currency, price)
      end

      def sell(currency:, price:)
        make_order('SELL', currency, price)
      end

      private

      def make_order(offer_type, currency, price)
        price = transaction_price(currency, price)
        symbol = "BTC#{currency.upcase}"

        params = {
          symbol: symbol,
          side: offer_type,
          type: 'MARKET',
          quoteOrderQty: price
        }

        request = signed_client.post('order') do |req|
          req.params = params
        end

        response = JSON.parse(request.body)

        parse_response(response)
      rescue StandardError
        Result::Failure.new('Could not make Binance order', RECOVERABLE)
      end

      def transaction_price(currency, price)
        limit = MIN_TRANSACTION_PRICES.fetch(currency.upcase.to_sym, DEFAULT_MIN_TRANSACTION_PRICE)
        [limit, price].max
      end

      def parse_response(response)
        return error_to_failure([response['msg']]) if response['msg'].present?

        rate = BigDecimal(response['cummulativeQuoteQty']) / BigDecimal(response['executedQty'])
        Result::Success.new(
          offer_id: response['orderId'],
          rate: rate,
          amount: response['executedQty']
        )
      end

      def unsigned_client
        Faraday.new(url: URL_BASE) do |conn|
          conn.adapter Faraday.default_adapter
        end
      end

      def signed_client
        Faraday.new(url: URL_BASE) do |conn|
          conn.headers['X-MBX-APIKEY'] = @api_key
          conn.use AddTimestamp
          conn.use AddSignature, @api_secret
          conn.adapter Faraday.default_adapter
        end
      end
    end
  end
end
