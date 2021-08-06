module ExchangeApi
  module Validators
    module Ftx
      class Validator < BaseValidator
        include ExchangeApi::Clients::Ftx

        def initialize(url_base:)
          @url = url_base + '/api/account'
        end

        def validate_credentials(api_key:, api_secret:)
          headers = if @url.include? 'us'
                      headers_us(api_key, api_secret, nil, '/api/account')
                    else
                      headers_eu(api_key, api_secret,nil, '/api/account')
                    end
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
