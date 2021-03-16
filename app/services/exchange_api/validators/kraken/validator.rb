require 'kraken_ruby_client'

module ExchangeApi
  module Validators
    module Kraken
      class Validator < BaseValidator
        include ExchangeApi::Clients::Kraken

        def validate_credentials(api_key:, api_secret:)
          @client = get_client(api_key, api_secret)
          response = @client.balance
          response['error'].none?
        rescue StandardError
          true
        end
      end
    end
  end
end
