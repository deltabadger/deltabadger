module ExchangeApi
  module Validators
    module Kraken
      class WithdrawalValidator < BaseValidator
        include ExchangeApi::Clients::Kraken

        def validate_credentials(api_key:, api_secret:)
          byebug
          @client = get_base_client(api_key, api_secret)
          response = @client.withdraw_cancel(refid: '9999999999', asset: 'XBT')
          'EFunding:Unknown reference id'.in?(response['error'])
        rescue StandardError
          false
        end
      end
    end
  end
end
