require 'kraken_ruby_client'

module ExchangeApi
  module Clients
    class Kraken
      def initialize(api_key:, api_secret:)
        @client =
          ::Kraken::Client.new(api_key: api_key, api_secret: api_secret)
      end

      def validate_credentials
        response = @client.balance

        return false if response.keys.include?('error')

        true
      end
    end
  end
end
