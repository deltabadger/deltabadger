module ExchangeApi
  module Clients
    module Binance

      AddTimestamp = Struct.new(:app, :api_secret) do
        def call(env)
          timestamp = DateTime.now.strftime('%Q')
          env.url.query = "#{env.url.query}&timestamp=#{timestamp}"
          app.call env
        end
      end

      AddSignature = Struct.new(:app, :api_secret) do
        def call(env)
          signature = OpenSSL::HMAC.hexdigest('sha256', api_secret, env.url.query)
          env.url.query = "#{env.url.query}&signature=#{signature}"
          app.call env
        end
      end

      def unsigned_client(url_base)
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

      def signed_client(api_key, api_secret, url_base)
        Faraday.new(url: url_base) do |conn|
          conn.headers['X-MBX-APIKEY'] = api_key
          conn.use AddTimestamp
          conn.use AddSignature, api_secret
          conn.adapter Faraday.default_adapter
        end
      end
    end
  end
end
