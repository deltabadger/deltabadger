require 'result'

module ExchangeApi
  module Traders
    module Bitso
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Bitso

        def initialize(
          api_key:,
          api_secret:,
          market: ExchangeApi::Markets::Bitso::Market.new,
          map_errors: ExchangeApi::MapErrors::Bitso.new
        )
          @api_key = api_key
          @api_secret = api_secret
          @market = market
          @map_errors = map_errors
        end

        private

        def place_order(order_params)
          path = '/v3/orders/'.freeze
          url = API_URL + path
          body = order_params.to_json
          request = Faraday.post(url, body, headers(@api_key, @api_secret, body, path, 'POST'))
          parse_request(request)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not make Bitso order', RECOVERABLE)
        end

        def transaction_price(symbol, price, force_smart_intervals)
          min_price = @market.minimum_order_price(symbol)
          return min_price unless min_price.success?

          return Result::Success.new(min_price.data) if force_smart_intervals

          quote_decimals = @market.quote_decimals(symbol)
          return quote_decimals unless quote_decimals.success?

          Result::Success.new([min_price.data, price].max.ceil(quote_decimals.data).to_d)
        end

        def smart_volume(symbol, price, rate, force_smart_intervals)
          volume = (price / rate).ceil(8)
          min_volume = @market.minimum_base_size(symbol)
          return min_volume unless min_volume.success?

          return Result::Success.new(min_volume.data) if force_smart_intervals

          Result::Success.new([min_volume.data, volume].max.to_d)
        end

        def common_order_params(symbol)
          {
            book: symbol
          }
        end

        def parse_request(request)
          response = JSON.parse(request.body)
          if request.status == 200 && request.reason_phrase == 'OK'
            response = response.fetch('payload')
            order_id = response.fetch('oid')
            parsed_params = get_order_by_id(order_id)
            return parsed_params unless parsed_params.success?

            Result::Success.new(parsed_params.data)
          else
            error_to_failure([response.fetch('error').fetch('code')])
          end
        end

        def sum_order_major(response)
          response.inject(0) { |sum, e| sum + e.fetch('major').to_f.abs }
        end

        def sum_order_minor(response)
          response.inject(0) { |sum, e| sum + e.fetch('minor').to_f.abs }
        end

        def get_order_by_id(order_id)
          path = "/v3/order_trades/#{order_id}".freeze
          url = API_URL + path
          request = Faraday.get(url, nil, headers(@api_key, @api_secret, nil, path, 'GET'))
          response = JSON.parse(request.body).fetch('payload')

          amount = sum_order_major(response)
          rate = (sum_order_minor(response) / amount)
          Result::Success.new(
            offer_id: order_id,
            amount: amount,
            rate: rate
          )
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch order parameters from Bitso')
        end
      end
    end
  end
end
