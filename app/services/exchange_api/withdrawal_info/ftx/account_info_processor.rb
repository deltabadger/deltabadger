require 'result'

module ExchangeApi
  module WithdrawalInfo
    module Ftx
      class AccountInfoProcessor < BaseAccountInfoProcessor
        include ExchangeApi::Clients::Ftx

        def initialize(
          api_key:,
          api_secret:,
          url_base:,
          market: ExchangeApi::Markets::Ftx::Market,
          map_errors: ExchangeApi::MapErrors::Ftx.new
        )
          @api_key = api_key
          @api_secret = api_secret
          @url_base = url_base
          @market = market.new(url_base: url_base)
          @base_client = base_client(url_base)
          @map_errors = map_errors
        end

        def withdrawal_currencies
          path = '/api/wallet/coins'.freeze
          url = @url_base + path
          headers = get_headers(url, @api_key, @api_secret, '', path, 'GET')
          request = @base_client.get(url, nil, headers)

          response = JSON.parse(request.body)
          all_symbols = response['result'].select { |c| c['canWithdraw'] }.map { |c| c['id'] }
          Result::Success.new(all_symbols)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch currencies from Ftx', RECOVERABLE)
        end

        def available_wallets
          path = '/api/wallet/saved_addresses'.freeze
          url = @url_base + path
          headers = get_headers(url, @api_key, @api_secret, '', path, 'GET')
          request = @base_client.get(url, nil, headers)

          response = JSON.parse(request.body)
          addresses = response['result'].map do |data|
            {
              currency: data['coin'],
              address: data['address']
            }
          end
          Result::Success.new(addresses)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch wallets from Ftx', RECOVERABLE)
        end

        def available_funds(currency)
          path = '/api/wallet/all_balances'.freeze
          url = @url_base + path
          headers = get_headers(url, @api_key, @api_secret, '', path, 'GET')
          request = @base_client.get(url, nil, headers)

          response = JSON.parse(request.body).fetch('result')
          coin_parameters = response.fetch('main').find { |c| c.fetch('coin') == currency } || { free: 0.0 }
          Result::Success.new(coin_parameters.fetch('free'))
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch funds from Ftx', RECOVERABLE)
        end
      end
    end
  end
end
