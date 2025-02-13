module ExchangeApi
  module Clients
    module Kucoin
      include BaseFaraday

      API_URL = 'https://api.kucoin.com'.freeze
      API_KEY_VERSION = '2'.freeze

      def headers(api_key, api_secret, passphrase, body, request_path, method = 'GET')
        timestamp = GetTimestamp.call
        signature = signature(api_secret, request_path, body, timestamp, method)
        signed_passphrase = sign_passphrase(api_secret, passphrase)
        {
          'KC-API-SIGN': signature,
          'KC-API-TIMESTAMP': timestamp,
          'KC-API-KEY': api_key,
          'KC-API-PASSPHRASE': signed_passphrase,
          'KC-API-KEY-VERSION': API_KEY_VERSION,
          'Content-Type': 'application/json'
        }
      end

      private

      def signature(api_secret, request_path = '', body = '', timestamp = nil, method = 'GET')
        body = body.to_json if body.is_a?(Hash)

        string_to_sign = "#{timestamp}#{method}#{request_path}#{body}"
        make_signature(api_secret, string_to_sign)
      end

      def sign_passphrase(api_secret, passphrase)
        make_signature(api_secret, passphrase)
      end

      def make_signature(api_secret, message)
        # create a sha256 hmac with the secret
        hash = OpenSSL::HMAC.digest('sha256', api_secret, message)
        Base64.strict_encode64(hash)
      end

      def base_client(url_base)
        Faraday.new(url: url_base, proxy: ENV.fetch('EU_PROXY_IP', nil))
      end

      def caching_client(url_base, expire_time = ENV['DEFAULT_MARKET_CACHING_TIME'])
        Faraday.new(url: url_base, proxy: ENV.fetch('EU_PROXY_IP', nil)) do |builder|
          builder.use :manual_cache,
                      expires_in: expire_time
          builder.adapter Faraday.default_adapter
        end
      end
    end
  end
end
