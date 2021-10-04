module ExchangeApi
  module Validators
    module Gemini
      class Validator < BaseValidator
        include ExchangeApi::Clients::Gemini

        URL = 'https://api.gemini.com/v1/notionalvolume'.freeze

        def validate_credentials(api_key:, api_secret:)
          body = {
            request: '/v1/notionalvolume',
            nonce: Time.now.strftime('%s%L')
          }

          body = body.to_json
          request = Faraday.post(URL, nil, headers(api_key, api_secret, body))
          return false if request.status != 200

          request.reason_phrase == 'OK'
        rescue StandardError
          false
        end
      end
    end
  end
end
