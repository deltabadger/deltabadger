module ExchangeApi
  module Clients
    module Bitso
      include BaseFaraday

      API_URL = 'https://api.bitso.com'.freeze

      def headers(api_key, api_secret, body, request_path, method = 'GET')
        nonce = GetTimestamp.call
        signature = build_signature(api_secret, request_path, body, nonce, method)
        {
          'Authorization': "Bitso #{api_key}:#{nonce}:#{signature}",
          'Content-Type': 'application/json'
        }
      end

      private

      def build_signature(api_secret, request_path = '', body = '', timestamp = nil, method = 'GET')
        body = body.to_json if body.is_a?(Hash)

        what = "#{timestamp}#{method}#{request_path}#{body}"

        # create a sha256 hmac with the secret
        OpenSSL::HMAC.hexdigest('sha256', api_secret, what)
      end
    end
  end
end
