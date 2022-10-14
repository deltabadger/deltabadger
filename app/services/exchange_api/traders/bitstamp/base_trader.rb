require 'result'

module ExchangeApi
  module Traders
    module Bitstamp
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Bitstamp

        def initialize(
          api_key:,
          api_secret:,
          market: ExchangeApi::Markets::Bitstamp::Market.new,
          map_errors: ExchangeApi::MapErrors::Bitstamp.new
        )
          @api_key = api_key
          @api_secret = api_secret
          @market = market
          @map_errors = map_errors
        end

        def fetch_order_by_id(_order_id, response_params = nil)
          Result::Success.new(response_params)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch order parameters from Bitstamp')
        end

        private

        def place_order(order_params, side, symbol, type)
          path = get_path(type, side, symbol)
          url = API_URL + path
          body = order_params.to_query
          request = Faraday.post(url, body, headers(@api_key, @api_secret, body, path, 'POST'))
          response = JSON.parse(request.body)

          parse_response(response)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not make Bitstamp order', **RECOVERABLE)
        end

        def smart_volume(symbol, price, rate, force_smart_intervals, smart_intervals_value, price_in_quote)
          volume_decimals = @market.base_decimals(symbol)
          return volume_decimals unless volume_decimals.success?

          volume = (!price_in_quote ? price : (price / rate)).ceil(volume_decimals.data)
          min_base = @market.minimum_base_size(symbol)
          return min_base unless min_base.success?

          smart_intervals_value = smart_intervals_value.nil? ? min_base.data : (smart_intervals_value / rate)
          smart_intervals_value = smart_intervals_value.ceil(volume_decimals.data)

          return Result::Success.new([smart_intervals_value, min_base.data].max) if force_smart_intervals

          Result::Success.new([min_base.data, volume].max.to_d)
        end

        def common_order_params
          {}
        end

        def parse_response(response)
          if error?(response)
            error_message = @map_errors.error_regex_mapping(response['reason'])
            return error_to_failure([error_message])
          end

          Result::Success.new(
            offer_id: response['id'],
            rate: response['price'],
            amount: response['amount']
          )
        end

        def sum_order_major(response)
          response.inject(0) { |sum, e| sum + e.fetch('major').to_d.abs }
        end

        def sum_order_minor(response)
          response.inject(0) { |sum, e| sum + e.fetch('minor').to_d.abs }
        end

        def error?(response)
          response['status'] && response['status'] == 'error'
        end

        def get_path(type, side, symbol)
          type == 'limit' ? "/api/v2/#{side}/#{symbol}/" : "/api/v2/#{side}/market/#{symbol}/"
        end
      end
    end
  end
end
