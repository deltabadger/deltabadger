module ExchangeApi
  module Clients
    module Coinbase
      include BaseFaraday

      def headers(api_key, api_secret, body, request_url, method = 'GET')
        if cdp_secret?(api_secret)
          jwt = jwt_token(api_key, api_secret, method, request_url)
          {
            'Authorization': "Bearer #{jwt}"
          }
        else
          # for legacy keys
          timestamp = Time.now.utc.to_i.to_s
          request_path = URI(request_url).path
          signature = signature(api_secret, timestamp, request_path, body, method)
          {
            'CB-ACCESS-KEY': api_key,
            'CB-ACCESS-TIMESTAMP': timestamp,
            'CB-ACCESS-SIGN': signature,
            'Accept': 'application/json',
            'Content-Type': 'application/json'
          }
        end
      end

      private

      def signature(api_secret, timestamp, request_path = '', body = '', method = 'GET')
        body = body.to_json if body.is_a?(Hash)
        payload = "#{timestamp}#{method}#{request_path}#{body}"

        # create a sha256 hmac with the secret
        OpenSSL::HMAC.hexdigest('sha256', api_secret, payload)
      end

      def jwt_token(api_key, api_secret, method, request_url)
        private_key = OpenSSL::PKey::EC.new(api_secret)
        request_host = URI(request_url).host
        path_url = URI(request_url).path
        uri = "#{method} #{request_host}#{path_url}"
        jwt_payload = {
          sub: api_key,
          iss: 'coinbase-cloud',
          nbf: Time.now.utc.to_i,
          exp: Time.now.utc.to_i + 120,
          uri: uri
        }
        JWT.encode(jwt_payload, private_key, 'ES256', { kid: api_key, nonce: SecureRandom.hex })
      end

      def cdp_secret?(api_secret)
        api_secret.start_with?('-----BEGIN EC PRIVATE KEY-----')
      end
    end
  end
end
