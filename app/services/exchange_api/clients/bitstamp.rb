module ExchangeApi
  module Clients
    module Bitstamp
      include BaseFaraday

      API_URL = 'https://www.bitstamp.net'.freeze
      CONTENT_TYPE = 'application/x-www-form-urlencoded'.freeze

      def headers(api_key, api_secret, body, request_path, method = 'GET')
        timestamp = GetTimestamp.call
        nonce = SecureRandom.uuid
        signature = build_signature(api_key, api_secret, request_path, body, nonce, timestamp, method)
        {
          'X-Auth': "BITSTAMP #{api_key}",
          'X-Auth-Signature': signature,
          'X-Auth-Nonce': nonce,
          'X-Auth-Timestamp': timestamp,
          'X-Auth-Version': 'v2',
          'Content-Type': CONTENT_TYPE
        }
      end

      private

      def build_signature(api_key, api_secret, request_path, body, nonce, timestamp, method)
        body = body.to_query if body.is_a?(Hash)

        string_to_sign = "BITSTAMP #{api_key}#{method}#{url_without_schema}#{request_path}"\
                         "#{CONTENT_TYPE}#{nonce}#{timestamp}v2#{body}"

        # create a sha256 hmac with the secret
        OpenSSL::HMAC.hexdigest('sha256', api_secret, string_to_sign).upcase
      end

      def url_without_schema
        API_URL[8...]
      end
    end
  end
end
