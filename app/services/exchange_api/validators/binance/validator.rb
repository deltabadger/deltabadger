module ExchangeApi
  module Validators
    module Binance
      class Validator < BaseValidator
        include ExchangeApi::Clients::Binance

        def initialize(url_base:)
          @url_base = url_base
        end

        ORDER_DOES_NOT_EXIST = -2011

        def validate_credentials(api_key:, api_secret:)
          client = signed_client(api_key, api_secret, @url_base)
          binance_log.info(client)
          binance_log.info("DELETE 'order' #{{symbol: 'ETHBTC', orderId: '9' * 10}}")
          request = client.delete(
            'order',
            symbol: 'ETHBTC',
            orderId: '9' * 10
          )
          response = JSON.parse(request.body)
          binance_log.info(response)
          response['code'] == ORDER_DOES_NOT_EXIST
        rescue StandardError => e
          binance_log.error(e.inspect)
          binance_log.error(e.backtrace)
          false
        end
      end
    end
  end
end
