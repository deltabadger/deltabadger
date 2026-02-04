module ExchangeApi
  module Clients
    module BaseFaraday
      def base_client(url_base, exchange: nil)
        proxy_url = proxy_url_for(exchange)
        Faraday.new(url: url_base) do |conn|
          conn.proxy = proxy_url if proxy_url.present?
          conn.adapter Faraday.default_adapter
        end
      end

      def caching_client(url_base, expire_time = ENV['DEFAULT_MARKET_CACHING_TIME'], exchange: nil)
        proxy_url = proxy_url_for(exchange)
        Faraday.new(url: url_base) do |builder|
          builder.proxy = proxy_url if proxy_url.present?
          builder.use :manual_cache,
                      expires_in: expire_time
          builder.adapter Faraday.default_adapter
        end
      end

      private

      def proxy_url_for(exchange)
        return nil unless exchange.present?

        ENV["PROXY_#{exchange.to_s.upcase}"]
      end
    end
  end
end
