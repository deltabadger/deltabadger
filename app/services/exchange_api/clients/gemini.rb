module ExchangeApi
  module Clients
    module Gemini
      include BaseFaraday

      def headers(api_key, api_secret, body)
        b64_body = Base64.strict_encode64(body)
        digest = OpenSSL::Digest.new('sha384')
        signature = OpenSSL::HMAC.hexdigest(digest, api_secret, b64_body)

        {
          'X-GEMINI-SIGNATURE': signature,
          'X-GEMINI-APIKEY': api_key,
          'X-GEMINI-PAYLOAD': b64_body,
          'Content-Type': 'text/plain',
          'Content-Length': '0',
          'Cache-Control': 'no-cache'
        }
      end
    end
  end
end
