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
        faraday_connector(url_base) do |conn|
          conn.headers['X-MBX-APIKEY'] = api_key
          conn.use AddTimestamp
          conn.use AddSignature, api_secret
          conn.adapter Faraday.default_adapter
        end
      end

      def faraday_connector(url_base)
        return Faraday.new(url: url_base, proxy: ENV.fetch('EU_PROXY_IP')) if url_base.in? [BinanceEnum::EU_URL_BASE, BinanceEnum::EU_WITHDRAWAL_URL_BASE]

        Faraday.new(url: url_base)
      end
    end
  end
end
