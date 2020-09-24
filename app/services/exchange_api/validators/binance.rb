module ExchangeApi
  module Validators
    class Binance < Base
      URL_BASE = 'https://api.binance.com/api/v3'.freeze

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

      private

      def signed_client(api_key, api_secret)
        Faraday.new(url: URL_BASE) do |conn|
          conn.headers['X-MBX-APIKEY'] = api_key
          conn.use AddTimestamp
          conn.use AddSignature, api_secret
          conn.adapter Faraday.default_adapter
        end
      end
    end
  end
end
