module ExchangeApi
  module Clients
    module CachingKraken
      def get_public(method, opts = {})
        url = "#{@api_public_url}#{method}"
        return Rails.cache.read(url) if Rails.cache.exist?(url)

        http = Curl.get(url, opts)
        Rails.cache.write(url, parse_response(http), expires_in: ENV['DEFAULT_CACHING_TIME'])
        parse_response(http)
      end
    end
  end
end

