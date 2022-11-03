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

      def headers(api_key, api_secret)
        access_token = get_token(api_key, api_secret)[:access_token]
        authorization = 'Bearer ' + access_token
        basic_headers.merge('Authorization': authorization)
      end

      private

      def token_headers(api_key, api_secret)
        auth_header = 'Basic ' + Base64.strict_encode64("#{api_key}:#{api_secret}")
        { 'Authorization': auth_header }.merge(basic_headers)
      end

      def basic_headers
        {
          'Content-Type': 'application/json',
          'User-Agent': 'Deltabadger'
        }
      end

      def token_body
        { "grant_type": 'client_credentials' }.to_json
      end
    end
  end
end
