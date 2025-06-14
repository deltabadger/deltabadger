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
        attributes = { url: url_base }
        if url_base.in?([BinanceEnum::EU_URL_BASE, BinanceEnum::EU_WITHDRAWAL_URL_BASE])
          attributes.merge!({ proxy: ENV['EU_HTTPS_PROXY'].present? ? "https://#{ENV['EU_HTTPS_PROXY']}" : nil })
        else
          attributes.merge!({ proxy: ENV['US_HTTPS_PROXY'].present? ? "https://#{ENV['US_HTTPS_PROXY']}" : nil })
        end

        attributes
      end
    end
  end
end
