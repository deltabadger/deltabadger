module ExchangeApi
  module Clients
    module Kraken
      private

      def get_client(api_key, api_secret)
        ::Kraken::Client.new(api_key: api_key, api_secret: api_secret)
      end
    end
  end
end
