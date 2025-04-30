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
          conn = Faraday.new(url: API_URL, proxy: ENV.fetch('EU_PROXY_IP', nil))
          request = conn.get(path, nil, headers(@api_key, @api_secret, @passphrase, '', path, 'GET'))
          response = JSON.parse(request.body)

          return Result::Failure.new('Waiting for KuCoin response', **NOT_FETCHED) unless order_done?(request, response)

          amount = response['data'].fetch('dealSize').to_f

          Result::Success.new(
            external_id: order_id,
            amount: amount,
            rate: (response['data'].fetch('dealFunds').to_f / amount)
          )
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch order parameters from KuCoin')
        end

        private

        def place_order(order_params)
          path = '/api/v1/orders'.freeze
          body = order_params.to_json
          conn = Faraday.new(url: API_URL, proxy: ENV.fetch('EU_PROXY_IP', nil))
          request = conn.post(path, body, headers(@api_key, @api_secret, @passphrase, body, path, 'POST'))

          parse_request(request)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not make KuCoin order', **RECOVERABLE)
        end

        def transaction_price(symbol, price, force_smart_intervals, smart_intervals_value, price_in_quote = true)
          min_price = price_in_quote ? @market.minimum_quote_size(symbol) : @market.minimum_base_size(symbol)
          return min_price unless min_price.success?

          price_decimals = price_in_quote ? @market.quote_decimals(symbol) : @market.base_decimals(symbol)
          return price_decimals unless price_decimals.success?

          smart_intervals_value = min_price.data if smart_intervals_value.nil?
          smart_intervals_value = smart_intervals_value.floor(price_decimals.data)

          return Result::Success.new([smart_intervals_value, min_price.data].max) if force_smart_intervals

          Result::Success.new([min_price.data, price.floor(price_decimals.data)].max)
        end

        def smart_volume(symbol, price, rate, force_smart_intervals, smart_intervals_value, price_in_quote = true)
          volume_decimals = @market.base_decimals(symbol)
          return volume_decimals unless volume_decimals.success?

          volume = (price_in_quote ? (price / rate) : price).ceil(volume_decimals.data)
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
            Result::Success.new(external_id: order_id)
          else
            error_to_failure([response.fetch('msg')])
          end
        end

        def order_done?(request, response)
          success?(request, response) && !response['data'].fetch('isActive')
        end

        def success?(request, response)
          request.status == 200 && response.fetch('code') == '200000'
        end
      end
    end
  end
end
