require 'kraken_ruby_client'

module ExchangeApi
  module Validators
    class Kraken < Base
      def validate_credentials(api_key:, api_secret:)
        @client = ::Kraken::Client.new(api_key: api_key, api_secret: api_secret)
        response = @client.balance
        response['error'].none?
      rescue StandardError
        false
      end
    end
  end
end
