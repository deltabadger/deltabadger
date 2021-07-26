module ExchangeApi
  module Clients
    module Bitbay
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

      def non_caching_client(url_base)
        Faraday.new(url: url_base) do |conn|
          conn.adapter Faraday.default_adapter
        end
      end

      def caching_client(url_base, expire_time = 30.second)
        Faraday.new(url: url_base) do |builder|
          builder.use :manual_cache,
                      expires_in: expire_time,
                      logger: Rails.logger
          builder.adapter Faraday.default_adapter
        end
      end

    end
  end
end
