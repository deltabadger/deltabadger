require 'result'

# rubocop#disable Style/StringLiterals
module ExchangeApi
  module Traders
    module Kucoin
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Kucoin

        def initialize(
          api_key:,
          api_secret:,
          passphrase:,
          market: ExchangeApi::Markets::Kucoin::Market.new,
          map_errors: ExchangeApi::MapErrors::Kucoin.new
        )
          @api_key = api_key
          @api_secret = api_secret
          @passphrase = passphrase
          @market = market
          @map_errors = map_errors
        end

        def fetch_order_by_id(order_id)
          path = "/api/v1/orders/#{order_id}".freeze
          url = API_URL + path
          request = Faraday.get(url, nil, headers(@api_key, @api_secret, @passphrase, '', path, 'GET'))
          response = JSON.parse(request.body)

          return Result::Failure.new('Waiting for KuCoin response', NOT_FETCHED) unless is_order_done?(request, response)
          return error_to_failure(['Order was canceled']) if canceled?(response)

          amount = response.fetch('size').to_f

          Result::Success.new(
            offer_id: order_id,
            amount: amount,
            rate: (response.fetch('price').to_f / amount)
          )
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch order parameters from KuCoin')
        end

        private

        def place_order(order_params)
          path = '/api/v1/orders'.freeze
          url = API_URL + path
          body = order_params.to_json
          request = Faraday.post(url, body, headers(@api_key, @api_secret, @passphrase, body, path, 'POST'))

          parse_request(request)
        rescue StandardError => e
          puts e
          Raven.capture_exception(e)
          Result::Failure.new('Could not make KuCoin order', RECOVERABLE)
        end

        def transaction_price(symbol, price, force_smart_intervals, smart_intervals_value)
          min_price = @market.minimum_quote_size(symbol)
          return min_price unless min_price.success?

          price_decimals = @market.quote_decimals(symbol)
          return price_decimals unless price_decimals.success?

          smart_intervals_value = min_price.data if smart_intervals_value.nil?
          smart_intervals_value = smart_intervals_value.floor(price_decimals.data)

          return Result::Success.new([smart_intervals_value, min_price.data].max) if force_smart_intervals

          Result::Success.new([min_price.data, price.floor(price_decimals.data)].max)
        end

        def smart_volume(symbol, price, rate, force_smart_intervals, smart_intervals_value)
          volume_decimals = @market.base_decimals(symbol)
          return volume_decimals unless volume_decimals.success?

          volume = (price / rate).ceil(volume_decimals.data)
          min_volume = @market.minimum_base_size(symbol)
          return min_volume unless min_volume.success?

          smart_intervals_value = min_volume.data if smart_intervals_value.nil?
          smart_intervals_value = smart_intervals_value.ceil(volume_decimals.data)

          return Result::Success.new([smart_intervals_value, min_volume.data].max) if force_smart_intervals

          Result::Success.new([min_volume.data, volume].max)
        end

        def common_order_params(symbol)
          {
            clientOid: SecureRandom.uuid,
            symbol: symbol
          }
        end

        def parse_request(request)
          response = JSON.parse(request.body)
          if success?(request, response)
            order_id = response['data'].fetch('orderId')
            Result::Success.new(offer_id: order_id)
          else
            error_to_failure([response.fetch('msg')])
          end
        end

        def is_order_done?(request, response)
          success?(request, response) && response.fetch('isActive') == false
        end

        def success?(request, response)
          request.status == 200 && response.fetch('code') == '200000'
        end
      end
    end
  end
end
