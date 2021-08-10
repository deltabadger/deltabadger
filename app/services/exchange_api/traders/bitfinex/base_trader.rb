require 'result'

# rubocop#disable Style/StringLiterals
module ExchangeApi
  module Traders
    module Bitfinex
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Bitfinex

        def initialize(
          api_key:,
          api_secret:,
          market: ExchangeApi::Markets::Bitfinex::Market.new,
          map_errors: ExchangeApi::MapErrors::Bitfinex.new
        )
          @api_key = api_key
          @api_secret = api_secret
          @market = market
          @map_errors = map_errors
        end

        def fetch_order_by_id(order_id)
          path = '/auth/r/orders/hist'.freeze
          url = PRIVATE_API_URL + path
          body = { id: [order_id] }.to_json
          request = Faraday.post(url, body, headers(@api_key, @api_secret, body, path))
          response = JSON.parse(request.body)

          status = response[0][13]
          return Result::Failure.new('Waiting for Bitfinex response', NOT_FETCHED) unless is_order_done?(status)
          return error_to_failure(['Order was canceled']) if cancelled?(response)

          amount = response[0][7].to_f
          price = response[0][16].to_f

          Result::Success.new(
            offer_id: order_id,
            amount: amount.abs,
            rate: price
          )
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch order parameters from Bitfinex')
        end

        private

        def place_order(order_params)
          path = '/auth/w/order/submit'.freeze
          url = PRIVATE_API_URL + path
          body = order_params.to_json
          request = Faraday.post(url, body, headers(@api_key, @api_secret, body, path))

          parse_request(request)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not make Bitfinex order', RECOVERABLE)
        end

        def smart_volume(symbol, price, rate, force_smart_intervals, smart_intervals_value)
          volume = (price / rate)
          min_volume = @market.minimum_order_size(symbol)
          return min_volume unless min_volume.success?

          smart_intervals_value = min_volume.data if smart_intervals_value.nil?
          smart_intervals_value = smart_intervals_value

          return Result::Success.new([smart_intervals_value, min_volume.data].max) if force_smart_intervals

          Result::Success.new([min_volume.data, volume].max)
        end

        def common_order_params(symbol)
          {
            symbol: "t#{symbol}"
          }
        end

        def parse_request(request)
          response = JSON.parse(request.body)
          if success?(response)
            order_id = response[4][0][0].to_s
            Result::Success.new(offer_id: order_id)
          else
            error_message = @map_errors.error_regex_mapping(response[2])
            error_to_failure([error_message])
          end
        end

        def is_order_done?(status)
          /EXECUTED @ .*/.match?(status)
        end

        def success?(response)
          response[0] != 'error'
        end

        def cancelled?(status)
          status == 'CANCELLED'
        end
      end
    end
  end
end
