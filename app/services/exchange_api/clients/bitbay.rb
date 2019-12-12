# rubocop#disable Style/StringLiterals
module ExchangeApi
  module Clients
    class Bitbay < ExchangeApi::Clients::Base
      def initialize(api_key:, api_secret:, map_errors: ExchangeApi::MapErrors::Bitbay.new)
        @api_key = api_key
        @api_secret = api_secret
        @map_errors = map_errors
      end

      def validate_credentials
        url = 'https://api.bitbay.net/rest/trading/history/transactions'
        response = Faraday.get(url, {}, headers(''))
        response.status == 200
      end

      def current_price(settings)
        url =
          "https://bitbay.net/API/Public/BTC#{settings.fetch('currency')}/ticker.json"
        response = JSON.parse(Faraday.get(url, {}, headers('')).body)

        bid = response.fetch('bid').to_f
        ask = response.fetch('ask').to_f

        (bid + ask) / 2
      end

      def buy(settings)
        puts 'Buying on bitbay'
        make_order('BUY', settings)
      end

      def sell(settings)
        puts 'selling on bitbay'
        make_order('SELL', settings)
      end

      private

      def make_order(offer_type, settings)
        currency = settings.fetch('currency')
        price = settings.fetch('price')

        url = "https://api.bitbay.net/rest/trading/offer/BTC-#{currency}"
        body = {
          offerType: offer_type,
          amount: nil,
          price: price,
          rate: nil,
          postOnly: false,
          mode: 'market',
          fillOrKill: false
        }.to_json

        response = JSON.parse(Faraday.post(url, body, headers(body)).body)
        parse_response(response)
      end

      def parse_response(response)
        if response.fetch('status') == 'Ok'
          Result::Success.new(
            offer_id: response.fetch('offerId'),
            rate: response.fetch('transactions').first.fetch('rate'),
            amount: response.fetch('transactions').first.fetch('amount')
          )
        else
          Result::Failure.new(
            *@map_errors.call(response.fetch('errors'))
          )
        end
      end

      def headers(body)
        timestamp = Time.now.to_i.to_s
        post = @api_key + timestamp.to_s + body.to_s
        signature = OpenSSL::HMAC.hexdigest('sha512', @api_secret, post)

        {
          'API-Key' => @api_key,
          'API-Hash' => signature,
          'operation-id' => SecureRandom.uuid.to_s,
          'Request-Timestamp' => timestamp,
          'Content-Type' => 'application/json'
        }
      end
    end
  end
end
