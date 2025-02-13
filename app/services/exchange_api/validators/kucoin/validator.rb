module ExchangeApi
  module Validators
    module Kucoin
      class Validator < BaseValidator
        include ExchangeApi::Clients::Kucoin
        FAKE_ORDER_ID = '9' * 10
        ORDER_NOT_FOUND_CODE = '400100'.freeze

        def validate_credentials(api_key:, api_secret:, passphrase:)
          path = "/api/v1/orders/#{FAKE_ORDER_ID}"
          conn = Faraday.new(url: API_URL, proxy: ENV.fetch('EU_PROXY_IP', nil))
          request = conn.delete(path, {}, headers(api_key, api_secret, passphrase, '', path, 'DELETE'))
          return false if request.status != 200

          response = JSON.parse(request.body)
          response['code'] == ORDER_NOT_FOUND_CODE
        rescue StandardError
          false
        end
      end
    end
  end
end
