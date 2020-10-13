module ExchangeApi
  module Validators
    module Binance
      class Validator < BaseValidator
        include ExchangeApi::Clients::Binance

        ORDER_DOES_NOT_EXIST = -2011

        def validate_credentials(api_key:, api_secret:)
          request = signed_client(api_key, api_secret).delete(
            'order',
            symbol: 'ETHBTC',
            orderId: '9' * 10
          )
          response = JSON.parse(request.body)
          response['code'] == ORDER_DOES_NOT_EXIST
        rescue StandardError
          false
        end
      end
    end
  end
end
