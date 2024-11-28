require 'result'
require 'csv'

module ExchangeApi
  module WithdrawalInfo
    module Kraken
      class AccountInfoProcessor < BaseAccountInfoProcessor
        include ExchangeApi::Clients::Kraken

        def initialize(
          api_key:,
          api_secret:,
          map_errors: ExchangeApi::MapErrors::Kraken.new
        )
          @client = get_base_client(api_key, api_secret)
          @caching_client = get_caching_client(api_key, api_secret)
          @map_errors = map_errors
        end

        def withdrawal_minimum(currency)
          result = fetch_minimum_withdrawal_amount(currency)
          return result if result.failure?

          result
        end

        def withdrawal_currencies
          response = @caching_client.assets
          return error_to_failure(response.fetch('error')) if response.fetch('error').any?

          all_symbols = response['result'].map { |_symbol, data| data['altname'] }
          Result::Success.new(all_symbols)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch currencies from Kraken', **RECOVERABLE)
        end

        def available_wallets
          # Kraken does not support retrieving information about crypto addresses
          Result::Success.new(nil)
        end

        def available_funds(bot)
          Rails.logger.info "withdrawal available_funds bot.currency: #{bot.currency}" # TODO: delete after testing
          minimum = withdrawal_minimum(bot.currency)
          Rails.logger.info "withdrawal available_funds minimum: #{minimum.inspect}" # TODO: delete after testing
          return minimum unless minimum.success?

          response = @client.withdraw_info(asset: bot.currency, key: bot.address, amount: minimum.data)
          Rails.logger.info "withdrawal available_funds response: #{response.inspect}" # TODO: delete after testing
          return error_to_failure(response.fetch('error')) if response.fetch('error').any?

          data = response.fetch('result').fetch('limit').to_f
          Rails.logger.info "withdrawal available_funds data: #{data.inspect}" # TODO: delete after testing
          Result::Success.new(data)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch funds from Kraken', **RECOVERABLE)
        end

        private

        def fetch_minimum_withdrawal_amount(currency)
          response = @client.withdraw_addresses(asset: currency)
          Rails.logger.info "withdrawal fetch_minimum_withdrawal_amount withdraw_addresses response: #{response.inspect}" # TODO: delete after testing
          return error_to_failure(response.fetch('error')) if response.fetch('error').any?

          # FIXME: We assume the user has only one withdrawal address for the asset, and one method
          method = response.fetch('result').first.fetch('method')
          response = @client.withdraw_methods(asset: currency, method: method)
          Rails.logger.info "withdrawal fetch_minimum_withdrawal_amount withdraw_methods response: #{response.inspect}" # TODO: delete after testing
          return error_to_failure(response.fetch('error')) if response.fetch('error').any?

          response.fetch('result').first.fetch('minimum').to_f
        end
      end
    end
  end
end
