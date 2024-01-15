module ExchangeApi
  module WithdrawalProcessors
    module Ftx
      class RequestProcessor < BaseRequestProcessor
        include ExchangeApi::Clients::Ftx

        def initialize(
          api_key:,
          api_secret:,
          url_base:,
          map_errors: ExchangeApi::MapErrors::Ftx.new,
          options: {}
        )
          @api_key = api_key
          @api_secret = api_secret
          @url_base = url_base
          @base_client = base_client(url_base)
          @map_errors = map_errors
          @options = options
        end

        def make_withdrawal(params)
          path = '/api/wallet/withdrawals'.freeze
          url = @url_base + path
          order_params = get_order_params(params)
          body = order_params.to_json

          headers = get_headers(url, @api_key, @api_secret, body, path, 'POST')
          request = @base_client.post(url, body, headers)
          response = JSON.parse(request.body)
          return error_to_failure([response['error']]) unless response['error'].nil?

          return Result::Failure.new('FTX withdrawal failed', **RECOVERABLE) if cancelled?(response)

          result = parse_withdrawal(response)
          Result::Success.new(result)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not make Kraken withdrawal', **RECOVERABLE)
        end

        private

        def get_order_params(params)
          {
            coin: params[:currency],
            size: params[:amount],
            address: params[:address]
          }
        end

        def cancelled?(response)
          response.fetch('result').fetch('status') == 'cancelled'
        end

        def parse_withdrawal(response)
          result = response.fetch('result')

          {
            offer_id: result.fetch('id'),
            amount: result.fetch('size')
          }
        end
      end
    end
  end
end
