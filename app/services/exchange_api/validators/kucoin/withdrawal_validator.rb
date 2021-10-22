module ExchangeApi
  module Validators
    module Kucoin
      class WithdrawalValidator < BaseValidator
        include ExchangeApi::Clients::Kucoin
        FAKE_ID = '9' * 10
        CANCEL_FAILED_CODE= '260010'.freeze

        def validate_credentials(api_key:, api_secret:, passphrase:)
          path = "/api/v1/withdrawals/#{FAKE_ID}"
          url = "#{API_URL}#{path}"
          request = Faraday.delete(url, {}, headers(api_key, api_secret, passphrase, '', path, 'DELETE'))
          return false if request.status != 200

          response = JSON.parse(request.body)
          response['code'] == CANCEL_FAILED_CODE
        rescue StandardError
          false
        end
      end
    end
  end
end
