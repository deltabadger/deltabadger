module ExchangeApi
  module Validators
    module Ftx
      class Validator < BaseValidator
        include ExchangeApi::Clients::Ftx

        def initialize(url_base:)
          @url = url_base + '/api/account'
        end

        def validate_credentials(api_key:, api_secret:)
          headers = get_headers(@url, api_key, api_secret,nil, '/api/account')
          request = Faraday.get(@url, nil, headers)
          return false if request.status != 200

          request.reason_phrase == 'OK'
        rescue StandardError
          false
        end
      end
    end
  end
end
