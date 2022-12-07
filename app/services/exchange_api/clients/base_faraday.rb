module ExchangeApi
  module Clients
    module BaseFaraday
      def base_client(url_base)
        binance_log.info("base_client(url_base)")
        Faraday.new(attributes(url_base)) do |conn|
          conn.adapter Faraday.default_adapter
        end
      rescue => e
        binance_log.info("=== ExchangeApi::Clients::BaseFaraday.base_client() rescue => e ===")
        binance_log.error(e.inspect)
      end

      def caching_client(url_base, expire_time = ENV['DEFAULT_MARKET_CACHING_TIME'])
        binance_log.info("caching_client(url_base, expire_time = ENV['DEFAULT_MARKET_CACHING_TIME'])")
        Faraday.new(attributes(url_base)) do |builder|
          builder.use :manual_cache,
                      expires_in: expire_time
          builder.adapter Faraday.default_adapter
        end
      rescue => e
        binance_log.info("=== ExchangeApi::Clients::BaseFaraday.caching_client() rescue => e ===")
        binance_log.error(e.inspect)
      end

      def attributes(url_base)
        attributes = { url: url_base }
        attributes.merge!({ proxy: ENV.fetch('EU_PROXY_IP') }) if url_base.in? [BinanceEnum::EU_URL_BASE, BinanceEnum::EU_WITHDRAWAL_URL_BASE]
      end

      def binance_log
        @binance_log ||= Logger.new("log/binance_log_7.log")
      end
    end
  end
end
