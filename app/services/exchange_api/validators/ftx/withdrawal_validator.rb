module ExchangeApi
  module Validators
    module Ftx
      class WithdrawalValidator < BaseValidator
        include ExchangeApi::Clients::Ftx

        def initialize(url_base:)
          @url = "#{url_base}/api/wallet/withdrawals"
        end

        def validate_credentials(api_key:, api_secret:)
          headers = get_headers(@url, api_key, api_secret, nil, '/api/wallet/withdrawals')
          conn = Faraday.new(proxy: ENV['US_HTTPS_PROXY'].present? ? "https://#{ENV['US_HTTPS_PROXY']}" : nil)
          request = conn.get(@url, nil, headers)
          return false if request.status != 200

          request.reason_phrase == 'OK'
        rescue StandardError
          false
        end
      end
    end
  end
end
