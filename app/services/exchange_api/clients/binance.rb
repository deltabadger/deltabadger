module ExchangeApi
  module Clients
    module Binance
      include BaseFaraday

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

      def signed_client(api_key, api_secret, url_base)
        Faraday.new(attributes(url_base)) do |conn|
          conn.headers['X-MBX-APIKEY'] = api_key
          conn.use AddTimestamp
          conn.use AddSignature, api_secret
          conn.adapter Faraday.default_adapter
        end
      end

      # FIXME: This is overwrites the default method in BaseFaraday. Seems to fix some issues with the EU proxy
      def base_client(url_base)
        Faraday.new(attributes(url_base)) do |conn|
          conn.adapter Faraday.default_adapter
        end
      end

      def caching_client(url_base, expire_time = ENV['DEFAULT_MARKET_CACHING_TIME'])
        Faraday.new(attributes(url_base)) do |builder|
          builder.use :manual_cache,
                      expires_in: expire_time
          builder.adapter Faraday.default_adapter
        end
      end

      def attributes(url_base)
        return url_base if ENV.fetch('EU_PROXY_IP', nil).blank?

        attributes = { url: url_base }
        attributes.merge!({ proxy: ENV.fetch('EU_PROXY_IP') }) if url_base.in? [BinanceEnum::EU_URL_BASE, BinanceEnum::EU_WITHDRAWAL_URL_BASE]

        attributes
      end
    end
  end
end
