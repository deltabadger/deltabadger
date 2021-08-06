module ExchangeApi
  module Clients
    module Bitfinex
      include BaseFaraday

      PUBLIC_API_URL = 'https://api-pub.bitfinex.com'.freeze
      PRIVATE_API_URL = 'https://api.bitfinex.com/v2'.freeze

      def headers(api_key, api_secret, body, request_path)
        timestamp = GetTimestamp.call
        signature = signature(api_secret, request_path, body, timestamp)
        {
          'bfx-nonce': timestamp,
          'bfx-apikey': api_key,
          'bfx-signature': signature,
          'Content-Type': 'application/json',
          'Accept': 'application/json'
        }
      end

      private

      def signature(api_secret, request_path = '', body = '', timestamp = nil)
        body = body.to_json if body.is_a?(Hash)
        string_to_sign = "/api/v2#{request_path}#{timestamp}#{body}"

        OpenSSL::HMAC.hexdigest('sha384', api_secret, string_to_sign)
      end
    end
  end
end
