require 'result'

# rubocop#disable Style/StringLiterals
module ExchangeApi
  module Traders
    module CoinbasePro
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::CoinbasePro

        def initialize(
          api_key:,
          api_secret:,
          passphrase:,
          market: ExchangeApi::Markets::CoinbasePro::Market.new,
          map_errors: ExchangeApi::MapErrors::CoinbasePro.new
        )
          @api_key = api_key
          @api_secret = api_secret
          @passphrase = passphrase
          @market = market
          @map_errors = map_errors
        end

        API_URL = 'https://api.pro.coinbase.com'.freeze

        def fetch_order_by_id(order_id)
          path = "/orders/#{order_id}".freeze
          url = API_URL + path
          request = Faraday.get(url, nil, headers(@api_key, @api_secret, @passphrase, '', path, 'GET'))
          response = JSON.parse(request.body)

          return Result::Failure.new('Waiting for Coinbase Pro response', NOT_FETCHED) unless is_order_done?(request, response)
          return error_to_failure(['Order was canceled']) if canceled?(response)

          amount = response.fetch('filled_size').to_f
          return Result::Failure.new('Waiting for Coinbase Pro response', NOT_FETCHED) unless filled?(amount)

          Result::Success.new(
            offer_id: order_id,
            amount: amount,
            rate: (response.fetch('executed_value').to_f / amount)
          )
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch order parameters from Coinbase')
        end

        private

        def place_order(order_params)
          path = '/orders'.freeze
          url = API_URL + path
          body = order_params.to_json
          request = Faraday.post(url, body, headers(@api_key, @api_secret, @passphrase, body, path, 'POST'))
          parse_request(request)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not make Coinbase order', RECOVERABLE)
        end

        def transaction_price(symbol, price, force_smart_intervals, smart_intervals_value, is_legacy)
          min_price = !is_legacy ? @market.minimum_base_size(symbol) : @market.minimum_order_price(symbol)
          return min_price unless min_price.success?

          price_decimals = !is_legacy ? @market.base_decimals(symbol) : @market.quote_decimals(symbol)
          return price_decimals unless price_decimals.success?

          smart_intervals_value = min_price.data if smart_intervals_value.nil?
          smart_intervals_value = smart_intervals_value.floor(price_decimals.data)

          return Result::Success.new([smart_intervals_value, min_price.data].max) if force_smart_intervals

          Result::Success.new([min_price.data, price.floor(price_decimals.data)].max)
        end

        def smart_volume(symbol, price, rate, force_smart_intervals, smart_intervals_value, is_legacy)
          volume_decimals = @market.base_decimals(symbol)
          return volume_decimals unless volume_decimals.success?

          volume = !is_legacy ? price.ceil(volume_decimals.data) : (price / rate).ceil(volume_decimals.data)
          min_volume = @market.minimum_base_size(symbol)
          return min_volume unless min_volume.success?

          smart_intervals_value = min_volume.data if smart_intervals_value.nil?
          smart_intervals_value = smart_intervals_value.ceil(volume_decimals.data)

          return Result::Success.new([smart_intervals_value, min_volume.data].max) if force_smart_intervals

          Result::Success.new([min_volume.data, volume].max)
        end

        def common_order_params(symbol, limit_only = false)
          {
            product_id: symbol
          }
        end

        def parse_request(request)
          response = JSON.parse(request.body)
          if success?(request)
            order_id = response.fetch('id')

            Result::Success.new(offer_id: order_id)
          else
            error_to_failure([response.fetch('message')])
          end
        end

        def is_order_done?(request, response)
          success?(request) && response.fetch('status') == 'done'
        end

        def success?(request)
          request.status == 200 && request.reason_phrase == 'OK'
        end

        def canceled?(response)
          response.fetch('done_reason', nil) == 'canceled'
        end

        def filled?(amount)
          amount != 0.0
        end
      end
    end
  end
end
