module ExchangeApi
  module Clients
    module Ftx
      def headers(api_key, api_secret, body, request_path, method = 'GET')
        timestamp = Time.now.strftime('%s%L')
        signature = signature(api_secret, request_path, body, timestamp, method)
        {
          'FTX-SIGN': signature,
          'FTX-TS': timestamp.to_s,
          'FTX-KEY': api_key,
          'Content-Type': 'application/json'
        }
      end

      private

      def signature(api_secret, request_path = '', body = '', timestamp = nil, method = 'GET')
        body = body.to_json if body.is_a?(Hash)

        what = "#{timestamp}#{method}#{request_path}#{body}"

        # create a sha256 hmac with the secret
        OpenSSL::HMAC.hexdigest('sha256', api_secret, what)
      end
    end
  end
end

