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

        def fetch_order_by_id(order_id)
          path = "/v3/order_trades/#{order_id}".freeze
          url = API_URL + path
          request = Faraday.get(url, nil, headers(@api_key, @api_secret, nil, path, 'GET'))
          return Result::Failure.new('Waiting for Bitso response', NOT_FETCHED) unless success?(request)

          response = JSON.parse(request.body).fetch('payload')
          amount = sum_order_major(response)
          return Result::Failure.new('Waiting for Bitso response', NOT_FETCHED) unless filled?(amount)

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

        def transaction_price(symbol, price, force_smart_intervals, smart_intervals_value)
          min_price = @market.minimum_order_price(symbol)
          return min_price unless min_price.success?

          quote_decimals = @market.quote_decimals(symbol)
          return quote_decimals unless quote_decimals.success?

          smart_intervals_value = min_price.data if smart_intervals_value.nil?
          smart_intervals_value = smart_intervals_value.ceil(quote_decimals.data)

          return Result::Success.new([smart_intervals_value, min_price.data].max) if force_smart_intervals

          Result::Success.new([min_price.data, price].max.ceil(quote_decimals.data).to_d)
        end

        def smart_volume(symbol, price, rate, force_smart_intervals, smart_intervals_value, price_in_quote)
          volume = price_in_quote ?  (price / rate).ceil(8) : price
          min_base = @market.minimum_base_size(symbol)
          return min_base unless min_base.success?

          min_quote = @market.minimum_order_price(symbol)
          return min_quote unless min_quote.success?

          min_volume = [min_base.data, (min_quote.data / rate).ceil(8)].max.to_d

          smart_intervals_value = smart_intervals_value.nil? ? min_volume : (smart_intervals_value / rate)
          smart_intervals_value = smart_intervals_value.ceil(8)
          return Result::Success.new([smart_intervals_value, min_volume].max) if force_smart_intervals

          Result::Success.new([min_volume, volume].max.to_d)
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

            Result::Success.new(offer_id: order_id)
          else
            error_to_failure([response.fetch('error').fetch('code')])
          end
        end

        def sum_order_major(response)
          response.inject(0) { |sum, e| sum + e.fetch('major').to_d.abs }
        end

        def sum_order_minor(response)
          response.inject(0) { |sum, e| sum + e.fetch('minor').to_d.abs }
        end

        def filled?(amount)
          amount != 0.0
        end

        def success?(request)
          JSON.parse(request.body).fetch('success')
        end
      end
    end
  end
end
