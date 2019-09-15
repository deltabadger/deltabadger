# rubocop#disable Style/StringLiterals
module ExchangeApi
  module Clients
    class Bitbay < ExchangeApi::Clients::Base
      def initialize(api_key:, api_secret:)
        @api_key = api_key
        @api_secret = api_secret
      end

      def validate_credentials
        url = 'https://api.bitbay.net/rest/trading/history/transactions'
        response = Faraday.get(url, {}, headers(''))
        response.status == 200
      end

      def buy(settings)
        # currency = settings.fetch("currency")
        puts "Buying on bitbay"
        currency = 'PLN'
        price = settings.fetch("price")

        url = "https://api.bitbay.net/rest/trading/offer/BTC-#{currency}"
        body = {
          offerType: 'SELL',
          amount: nil,
          price: price,
          rate: nil,
          postOnly: false,
          mode: 'market',
          fillOrKill: false
        }.to_json

        if data.fetch('status') == 'Ok'
          Result::Success.new(
            offer_id: response.fetch('offerId'),
            rate: response.fetch('rate'),
            amount: response.fetch('amount')
          )
        else
          Result::Failure.new('Something went wrong!')
        end
      end

      def sell(settings)
        # currency = settings.fetch("currency")
        currency = 'PLN'
        price = settings.fetch("price")
        puts "selling on bitbay"

        url = "https://api.bitbay.net/rest/trading/offer/BTC-#{currency}"
        body = {
          offerType: 'SELL',
          amount: nil,
          price: price,
          rate: nil,
          postOnly: false,
          mode: 'market',
          fillOrKill: false
        }.to_json

        response = JSON.parse(Faraday.post(url, body, headers(body)).body)

        if data.fetch('status') == 'Ok'
          Result::Success.new(
            offer_id: response.fetch('offerId'),
            rate: response.fetch('rate'),
            amount: response.fetch('amount')
          )
        else
          Result::Failure.new('Something went wrong!')
        end

        true
      end

      def offers
        url = 'https://api.bitbay.net/rest/trading/history/transactions'
        params = {
          'markets' => ['BTC-PLN'],
          'nextPageCursor' => 'start'
        }
        Faraday.get(url, params, headers(''))
      end

      private

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
