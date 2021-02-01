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

        def transaction_price(symbol, price, force_smart_intervals)
          min_price = @market.minimum_order_price(symbol)
          return min_price unless min_price.success?

          return Result::Success.new(min_price.data) if force_smart_intervals

          Result::Success.new([min_price.data, price].max)
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
            product_id: symbol
          }
        end

        def parse_request(request)
          response = JSON.parse(request.body)
          if request.status == 200 && request.reason_phrase == 'OK'
            order_id = response.fetch('id')
            parsed_params = get_order_by_id(order_id)
            return parsed_params unless parsed_params.success?

            Result::Success.new(parsed_params.data)
          else
            error_to_failure([response.fetch('message')])
          end
        end

        def is_order_done?(response)
          response.fetch('status') == 'done'
        end

        def get_order_by_id(order_id)
          sleep(4.0)
          path = "/orders/#{order_id}".freeze
          url = API_URL + path
          request = Faraday.get(url, nil, headers(@api_key, @api_secret, @passphrase, '', path, 'GET'))
          response = JSON.parse(request.body)

          amount = response.fetch('filled_size').to_f
          Result::Success.new(
            offer_id: order_id,
            amount: amount,
            rate: (response.fetch('executed_value').to_f / amount)
          )
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch order parameters from Coinbase')
        end
      end
    end
  end
end
