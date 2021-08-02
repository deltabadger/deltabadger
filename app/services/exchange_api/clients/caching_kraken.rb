module ExchangeApi
  module Clients
    module CachingKraken
      def get_public(method, opts = {})
        url = "#{@api_public_url}#{method}"
        cache_key = url + "/#{opts.to_s}"
        return Rails.cache.read(cache_key) if Rails.cache.exist?(cache_key)

        http = Curl.get(url, opts)
        parsed_response = parse_response(http)
        Rails.cache.write(cache_key, parsed_response, expires_in: ENV['DEFAULT_MARKET_CACHING_TIME'])
        parsed_response
      end
    end
  end
end

