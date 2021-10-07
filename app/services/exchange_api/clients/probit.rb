module ExchangeApi
  module Clients
    module Probit
      include BaseFaraday
      API_URL = 'https://api.probit.com'.freeze
      AUTH_API_URL = 'https://accounts.probit.com/token'.freeze

      def get_token(api_key, api_secret)
        response = Faraday.post(AUTH_API_URL, token_body, token_headers(api_key, api_secret))
        {
          status: response.status,
          access_token: JSON.parse(response.body)['access_token']
        }
      end

      def token_headers(api_key, api_secret)
        auth_header = 'Basic ' + Base64.strict_encode64("#{api_key}:#{api_secret}")
        { 'Content-Type': 'application/json',
          'Authorization': auth_header }
      end

      def token_body
        { "grant_type": 'client_credentials' }.to_json
      end

      def headers(api_key,api_secret, body, request_path, method = 'GET')
        timestamp = GetTimestamp.call
      end
    end
  end
end
