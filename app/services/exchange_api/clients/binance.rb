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
        Faraday.new(url: url_base) do |conn|
          conn.headers['X-MBX-APIKEY'] = api_key
          conn.use AddTimestamp
          conn.use AddSignature, api_secret
          conn.adapter Faraday.default_adapter
        end
      end
      def faraday_connector(url_base)

        binance_log.info("=== faraday_connector ===")
        binance_log.info(url_base)
        binance_log.info(url_base.in? [BinanceEnum::EU_URL_BASE, BinanceEnum::EU_WITHDRAWAL_URL_BASE])
        binance_log.info(ENV.fetch('EU_PROXY_IP'))

        connector = if url_base.in? [BinanceEnum::EU_URL_BASE, BinanceEnum::EU_WITHDRAWAL_URL_BASE]
                      Faraday.new(url: url_base, proxy: ENV.fetch('EU_PROXY_IP'))
                    else
                      Faraday.new(url: url_base)
                    end

        binance_log.info("connector")
        binance_log.info(connector)

        connector
      end

      def binance_log
        @binance_log ||= Logger.new("log/binance_log_2.log")
      end
    end
  end
end
