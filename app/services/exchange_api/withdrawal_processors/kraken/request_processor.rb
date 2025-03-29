module ExchangeApi
  module WithdrawalProcessors
    module Kraken
      class RequestProcessor < BaseRequestProcessor
        include ExchangeApi::Clients::Kraken

        def initialize(
          api_key:,
          api_secret:,
          market: ExchangeApi::Markets::Kraken::Market.new,
          map_errors: ExchangeApi::MapErrors::Kraken.new,
          options: {}
        )
          @client = get_base_client(api_key, api_secret)
          @market = market
          @map_errors = map_errors
          @options = options
        end

        def make_withdrawal(params)
          response = @client.withdraw(asset: params[:currency], key: params[:address], amount: params[:amount])
          return error_to_failure(response.fetch('error')) if response.fetch('error').any?

          external_id = get_external_id(response)
          Rails.logger.info "Kraken withdrawal external_id: #{external_id}"
          response = @client.withdraw_status(asset: params[:currency])
          return error_to_failure(response.fetch('error')) if response.fetch('error').any?

          withdrawal_details = response.fetch('result').find { |w| w['refid'] == external_id }
          return Result::Failure.new("Kraken withdrawal #{external_id} failed", **RECOVERABLE) if failed?(withdrawal_details)

          result = parse_withdrawal(withdrawal_details).merge(external_id: external_id)
          Result::Success.new(result)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not make Kraken withdrawal', **RECOVERABLE)
        end

        private

        def get_external_id(response)
          created_order = response.fetch('result')
          created_order.fetch('refid')
        end

        def parse_withdrawal(withdrawal_details)
          {
            amount: withdrawal_details.fetch('amount')
          }
        end

        def failed?(withdrawal_details)
          withdrawal_details.fetch('status') == 'Failure'
        end
      end
    end
  end
end
