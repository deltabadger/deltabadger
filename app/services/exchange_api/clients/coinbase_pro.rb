module ExchangeApi
  module Clients
    module CoinbasePro
      include BaseFaraday
      def headers(api_key, api_secret, passphrase, body, request_path, method = 'GET')
        timestamp = Time.now.utc.to_i.to_s
        signature = signature(api_secret, request_path, body, timestamp, method)
        {
          'CB-ACCESS-SIGN': signature,
          'CB-ACCESS-TIMESTAMP': timestamp,
          'CB-ACCESS-KEY': api_key,
          'CB-ACCESS-PASSPHRASE': passphrase,
          'Content-Type': 'application/json'
        }
      end

      private

      def signature(api_secret, request_path = '', body = '', timestamp = nil, method = 'GET')
        body = body.to_json if body.is_a?(Hash)
        timestamp ||= Time.now.to_i

        what = "#{timestamp}#{method}#{request_path}#{body}"

        # create a sha256 hmac with the secret
        secret = Base64.decode64(api_secret)
        hash = OpenSSL::HMAC.digest('sha256', secret, what)
        Base64.strict_encode64(hash)
      end
    end
  end
end

