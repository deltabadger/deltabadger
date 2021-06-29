require 'result'

module ExchangeApi
  module Traders
    module Ftx
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Ftx

        def initialize(
          api_key:,
          api_secret:,
          market: ExchangeApi::Markets::Ftx::Market.new,
          map_errors: ExchangeApi::MapErrors::Ftx.new
        )
          @api_key = api_key
          @api_secret = api_secret
          @market = market
          @map_errors = map_errors
        end

        def fetch_order_by_id(order_id)
          path = "/api/orders/#{order_id}".freeze
          url = API_URL + path
          request = Faraday.get(url, nil, headers(@api_key, @api_secret, '', path, 'GET'))
          return Result::Failure.new('Waiting for FTX response', NOT_FETCHED) unless success?(request)

          response = JSON.parse(request.body).fetch('result')
          return Result::Failure.new('Waiting for FTX response', NOT_FETCHED) unless closed?(response)

          # FTX does not return your fund, just closes order
          return error_to_failure(['Not enough balances']) if insufficient_funds?(response)

          amount = response.fetch('filledSize').to_f
          rate = response.fetch('avgFillPrice').to_f

          Result::Success.new(
            offer_id: order_id,
            amount: amount,
            rate: rate
          )
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch order parameters from FTX')
        end

        private

        def place_order(order_params)
          path = '/api/orders'.freeze
          url = API_URL + path
          body = order_params.to_json

          request = Faraday.post(url, body, headers(@api_key, @api_secret, body, path, 'POST'))
          parse_request(request)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not make FTX order', RECOVERABLE)
        end

        def smart_volume(symbol, price, rate, force_smart_intervals, smart_intervals_value)
          volume_decimals = @market.base_decimals(symbol)
          return volume_decimals unless volume_decimals.success?

          volume = (price / rate).floor(volume_decimals.data)
          min_volume = @market.minimum_base_size(symbol)
          return min_volume unless min_volume.success?

          smart_intervals_value = min_volume.data if smart_intervals_value.nil?
          smart_intervals_value = smart_intervals_value.floor(volume_decimals.data)

          return Result::Success.new([smart_intervals_value, min_volume.data].max) if force_smart_intervals

          Result::Success.new([min_volume.data, volume].max)
        end

        def common_order_params(symbol)
          {
            market: symbol
          }
        end

        def parse_request(request)
          response = JSON.parse(request.body)

          if was_filled?(request)
            response = response.fetch('result')
            order_id = response.fetch('id')

            Result::Success.new(offer_id: order_id)
          else
            error_to_failure([response.fetch('error')])
          end
        end

        def was_filled?(request)
          request.status == 200 && request.reason_phrase == 'OK'
        end

        def insufficient_funds?(response)
          response.fetch('status') == 'closed' && response.fetch('filledSize', 0).to_f == 0.0
        end

        def closed?(response)
          response.fetch('status') == 'closed'
        end

        def success?(request)
          JSON.parse(request.body).fetch('success')
        end
      end
    end
  end
end
