module ExchangeApi
  module Validators
    class Bitbay < Base
      URL = 'https://api.bitbay.net/rest/trading/history/transactions'.freeze

      def validate_credentials(api_key, api_secret)
        request = Faraday.get(URL, {}, verify_headers(api_key, api_secret))
        return false if request.status != 200

        response = JSON.parse(request.body)
        response['status'] == 'Ok'
      rescue StandardError
        false
      end

      private

      def verify_headers(api_key, api_secret)
        timestamp = Time.now.to_i.to_s
        post = api_key + timestamp.to_s
        signature = OpenSSL::HMAC.hexdigest('sha512', api_secret, post)
        {
          'API-Key' => api_key,
          'API-Hash' => signature,
          'operation-id' => SecureRandom.uuid.to_s,
          'Request-Timestamp' => timestamp,
          'Content-Type' => 'application/json'
        }
      end
    end
  end
end
