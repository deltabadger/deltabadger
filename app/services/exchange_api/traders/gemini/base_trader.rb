require 'result'

# rubocop#disable Style/StringLiterals
module ExchangeApi
  module Traders
    module Gemini
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Gemini

        def initialize(
          api_key:,
          api_secret:,
          market: ExchangeApi::Markets::Gemini::Market.new,
          map_errors: ExchangeApi::MapErrors::Gemini.new
        )
          @api_key = api_key
          @api_secret = api_secret
          @market = market
          @map_errors = map_errors
        end

        API_URL = 'https://api.gemini.com'.freeze

        private

        def place_order(order_params)
          path = '/v1/order/new'.freeze
          url = API_URL + path
          order_params = order_params.merge(request: path, nonce: Time.now.strftime('%s%L'))
          body = order_params.to_json
          request = Faraday.post(url, nil, headers(@api_key, @api_secret, body))
          parse_request(request)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not make Gemini order', RECOVERABLE)
        end

        def smart_volume(symbol, price, rate, force_smart_intervals)
          volume_decimals = @market.base_decimals(symbol)
          return volume_decimals unless volume_decimals.success?

          volume = (price / rate).ceil(volume_decimals.data)
          min_volume = @market.minimum_base_size(symbol)
          return min_volume unless min_volume.success?

          return Result::Success.new(min_volume.data) if force_smart_intervals

          Result::Success.new([min_volume.data, volume].max)
        end

        def common_order_params(symbol)
          {
            symbol: symbol
          }
        end

        def parse_request(request)
          response = JSON.parse(request.body)
          return Result::Failure.new('Could not make Gemini order', RECOVERABLE) if was_not_filled?(response)

          if request.status == 200 && request.reason_phrase == 'OK'
            order_id = response.fetch('order_id')
            amount = response.fetch('executed_amount').to_f

            Result::Success.new(
              offer_id: order_id,
              amount: amount,
              rate: response.fetch('avg_execution_price').to_f.round(2)
            )
          else
            error_to_failure([response.fetch('reason')])
          end
        end

        def was_not_filled?(response)
          response.fetch('reason', '') == 'FillOrKillWouldNotFill'
        end
      end
    end
  end
end
