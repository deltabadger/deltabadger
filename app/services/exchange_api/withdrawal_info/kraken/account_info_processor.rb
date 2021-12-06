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
          market: ExchangeApi::Markets::Kraken::Market.new,
          map_errors: ExchangeApi::MapErrors::Kraken.new
        )
          @client = get_base_client(api_key, api_secret)
          @caching_client = get_caching_client(api_key, api_secret)
          @market = market
          @map_errors = map_errors
        end

        def withdrawal_minimum(currency)
          data = fetch_minimum_fee_data(currency)
          return Result::Failure.new('Kraken withdrawal minimum not found on list') unless data.present?

          Result::Success.new(data['Minimum'].to_f)
        end

        def withdrawal_fee(currency)
          data = fetch_minimum_fee_data(currency)
          return Result::Failure.new('Kraken withdrawal fee not found on list') unless data.present?

          Result::Success.new(data['Fee'].to_f)
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

        def available_funds(bot)
          minimum = withdrawal_minimum(bot.currency)
          return minimum unless minimum.success?

          response = @client.withdraw_info(asset: bot.currency, key: bot.address, amount: minimum.data)
          return error_to_failure(response.fetch('error')) if response.fetch('error').any?

          data = response.fetch('result').fetch('limit').to_f
          Result::Success.new(data)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch funds from Kraken', RECOVERABLE)
        end

        private

        def fetch_minimum_fee_data(currency)
          csv_text = File.read(File.expand_path('../kraken_minimums_and_fees.csv', __FILE__))
          minimums_fees_csv = CSV.parse(csv_text, :headers => true)
          minimums_fees_csv.each.find { |row| row['Asset'] == currency }
        end
      end
    end
  end
end
