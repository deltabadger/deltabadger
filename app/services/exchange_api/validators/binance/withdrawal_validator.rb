module ExchangeApi
  module Validators
    module Binance
      class WithdrawalValidator < BaseValidator
        include ExchangeApi::Clients::Binance

        def initialize(url_base:)
          @url_base = url_base
        end

        def validate_credentials(api_key:, api_secret:)
          @url_base.include?('binance.us') ? validate_us(api_key, api_secret) : validate_eu(api_key, api_secret)
        rescue StandardError
          false
        end

        private

        def validate_eu(api_key, api_secret)
          request = signed_client(api_key, api_secret, @url_base).get('account/apiRestrictions')
          response = JSON.parse(request.body)
          response['enableWithdrawals']
        end

        def validate_us(api_key, api_secret)
          # request = signed_client(api_key, api_secret, @url_base).get('depositAddress.html')
          # response = JSON.parse(request.body)
          # response['enableWithdrawals']
          true
        end
      end
    end
  end
end
