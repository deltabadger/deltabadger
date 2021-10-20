require 'result'

module ExchangeApi
  module WithdrawalInfo
    module Kraken
      class AccountInfoProcessor < BaseAccountInfoProcessor
        include ExchangeApi::Clients::Kraken

        def initialize(
          api_key:,
          api_secret:,
          market: ExchangeApi::Markets::Kraken::Market.new,
          map_errors: ExchangeApi::MapErrors::Kraken.new,
          options: {}
        )
          @client = get_base_client(api_key, api_secret)
          @caching_client = get_caching_client(api_key, api_secret)
          @market = market
          @map_errors = map_errors
          @options = options
        end

        def withdrawal_currencies
          response = @caching_client.assets
          return error_to_failure(response.fetch('error')) if response.fetch('error').any?

          all_symbols = response['result'].map { |_symbol, data| data['altname'] }
          Result::Success.new(all_symbols)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch currencies from Kraken', RECOVERABLE)
        end

        def available_wallets
          # Kraken does not support retrieving information about crypto addresses
          Result::Success.new(nil)
        end

        def available_funds(currency)
          response = @client.balance
          return error_to_failure(response.fetch('error')) if response.fetch('error').any?

          response.fetch('result').fetch(currency, '0.0')
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch funds from Kraken', RECOVERABLE)
        end
      end
    end
  end
end
