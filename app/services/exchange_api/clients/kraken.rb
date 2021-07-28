module ExchangeApi
  module Clients
    module Kraken
      private

      def get_base_client(api_key, api_secret)
        ::Kraken::Client.new(api_key: api_key, api_secret: api_secret)
      end

      def get_caching_client(api_key, api_secret)
        client = ::Kraken::Client.new(api_key: api_key, api_secret: api_secret)
        client.extend(CachingKraken)
        client
      end
    end
  end
end
