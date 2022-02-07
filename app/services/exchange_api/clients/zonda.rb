module ExchangeApi
  module Clients
    module Zonda
      include BaseFaraday

      def headers(api_key, api_secret, body)
        timestamp = Time.now.to_i.to_s
        post = api_key + timestamp.to_s + body.to_s
        signature = OpenSSL::HMAC.hexdigest('sha512', api_secret, post)
        {
          'API-Key' => api_key,
          'API-Hash' => signature,
          'operation-id' => SecureRandom.uuid.to_s,
          'Request-Timestamp' => timestamp,
          'Content-Type' => 'application/json'
        }
      end
    end
  end
end
