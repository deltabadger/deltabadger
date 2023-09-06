module ExchangeApi
  module Clients
    module Coinbase
      include BaseFaraday

      def headers(api_key, api_secret, body, request_path, method = 'GET')
        timestamp = Time.now.utc.to_i.to_s
        signature = signature(api_secret, timestamp, request_path, body, method)
        {
          'CB-ACCESS-KEY': api_key,
          'CB-ACCESS-TIMESTAMP': timestamp,
          'CB-ACCESS-SIGN': signature,
          'Accept': 'application/json',
          'Content-Type': 'application/json'
        }
      end

      private

      def signature(api_secret, timestamp, request_path = '', body = '', method = 'GET')
        body = body.to_json if body.is_a?(Hash)
        payload = "#{timestamp}#{method}#{request_path}#{body}"

        # create a sha256 hmac with the secret
        OpenSSL::HMAC.hexdigest('sha256', api_secret, payload)
      end
    end
  end
end
