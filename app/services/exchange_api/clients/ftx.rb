module ExchangeApi
  module Clients
    module Ftx
      include BaseFaraday

      def get_headers(url, api_key, api_secret, body, request_path, method = 'GET', use_subaccount, selected_subaccount)
        if url.include? 'ftx.us'
          headers_us(api_key, api_secret, body, request_path, method, use_subaccount, selected_subaccount)
        else
          headers_eu(api_key, api_secret, body, request_path, method, use_subaccount, selected_subaccount)
        end
      end

      private

      def headers_eu(api_key, api_secret, body, request_path, method, use_subaccount, selected_subaccount)
        timestamp = GetTimestamp.call
        signature = build_signature(api_secret, request_path, body, timestamp, method)
        {
          'FTX-SIGN': signature,
          'FTX-TS': timestamp,
          'FTX-KEY': api_key,
          'Content-Type': 'application/json',
          'FTX-SUBACCOUNT': use_subaccount ? selected_subaccount : nil
        }.compact
      end

      def headers_us(api_key, api_secret, body, request_path, method, use_subaccount, selected_subaccount)
        timestamp = GetTimestamp.call
        signature = build_signature(api_secret, request_path, body, timestamp, method)
        {
          'FTXUS-SIGN': signature,
          'FTXUS-TS': timestamp,
          'FTXUS-KEY': api_key,
          'Content-Type': 'application/json',
          'FTXUS-SUBACCOUNT': use_subaccount ? selected_subaccount : nil
        }.compact
      end

      def build_signature(api_secret, request_path = '', body = '', timestamp = nil, method = 'GET')
        body = body.to_json if body.is_a?(Hash)

        what = "#{timestamp}#{method}#{request_path}#{body}"

        # create a sha256 hmac with the secret
        OpenSSL::HMAC.hexdigest('sha256', api_secret, what)
      end
    end
  end
end
