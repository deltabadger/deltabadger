module ExchangeApi
  module Validators
    module CoinbasePro
      class WithdrawalValidator < BaseValidator
        include ExchangeApi::Clients::CoinbasePro

        URL = 'https://api.exchange.coinbase.com/withdrawals/fee-estimate'.freeze

        def validate_credentials(api_key:, api_secret:, passphrase:)
          body = { crypto_address: '999999999', currency: 'BTC' }
          request = Faraday.get(URL, body, headers(api_key, api_secret, passphrase, '',
                                                   '/withdrawals/fee-estimate?' + body.to_query))
          response = JSON.parse(request.body)

          response['message'] == 'Invalid BTC address: 999999999'
        rescue StandardError
          false
        end
      end
    end
  end
end
