require 'result'

# rubocop#disable Style/StringLiterals
module ExchangeApi
  module Traders
    module Coinbase
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Coinbase

        def initialize(
          api_key:,
          api_secret:,
          market: ExchangeApi::Markets::Coinbase::Market.new,
          map_errors: ExchangeApi::MapErrors::Coinbase.new
        )
          @api_key = api_key
          @api_secret = api_secret
          @market = market
          @map_errors = map_errors
        end

        API_URL = 'https://api.coinbase.com'.freeze

        # rubocop:disable Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity
        def fetch_order_by_id(order_id, retry_attempts = 0)
          path = "/api/v3/brokerage/orders/historical/#{order_id}".freeze
          url = API_URL + path
          response = Faraday.get(url, nil, headers(@api_key, @api_secret, '', path, 'GET'))

          Rails.logger.info "Response status: #{response.status}"
          Rails.logger.info "Response body: #{response.body}"

          if response.status.between?(200, 299)
            parsed_response = JSON.parse(response.body)
            amount = parsed_response.dig('order', 'filled_size')&.to_f
            rate = parsed_response.dig('order', 'average_filled_price')&.to_f

            if amount.nil? || rate.nil? || (market_order?(parsed_response) && !filled?(parsed_response))
              Rails.logger.info 'Waiting for Coinbase response'
              sleep 0.5
              return fetch_order_by_id(order_id)
            end
            Result::Success.new(
              offer_id: order_id,
              amount: amount,
              rate: rate
            )
          elsif response.status == 404 && retry_attempts < 10
            Rails.logger.info 'Coinbase order not found (yet). Retrying...'
            sleep 0.5
            fetch_order_by_id(order_id, retry_attempts + 1)
          else
            Raven.capture_exception(StandardError.new("Unexpected response status: #{response.status}"))
            Result::Failure.new("Could not fetch order parameters from Coinbase. Unexpected response status: #{response.status}")
          end
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not fetch order parameters from Coinbase')
        end
        # rubocop:enable Metrics/CyclomaticComplexity, Metrics/PerceivedComplexity

        private

        def place_order(order_params)
          path = '/api/v3/brokerage/orders'.freeze
          url = API_URL + path
          body = order_params.to_json
          request = Faraday.post(url, body, headers(@api_key, @api_secret, body, path, 'POST'))
          parse_request(request)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not make Coinbase order', **RECOVERABLE)
        end

        def transaction_price(symbol, price, force_smart_intervals, smart_intervals_value, price_in_quote)
          min_price = price_in_quote ? @market.minimum_order_price(symbol) : @market.minimum_base_size(symbol)
          return min_price unless min_price.success?

          price_decimals = price_in_quote ? @market.quote_decimals(symbol) : @market.base_decimals(symbol)
          return price_decimals unless price_decimals.success?

          smart_intervals_value = min_price.data if smart_intervals_value.nil?
          smart_intervals_value = smart_intervals_value.floor(price_decimals.data)

          return Result::Success.new([smart_intervals_value, min_price.data].max) if force_smart_intervals

          Result::Success.new([min_price.data, price.floor(price_decimals.data)].max)
        end

        def smart_volume(symbol, price, rate, force_smart_intervals, smart_intervals_value, price_in_quote)
          volume_decimals = @market.base_decimals(symbol)
          return volume_decimals unless volume_decimals.success?

          volume = price_in_quote ? (price / rate).ceil(volume_decimals.data) : price.ceil(volume_decimals.data)
          min_volume = @market.minimum_base_size(symbol)
          return min_volume unless min_volume.success?

          smart_intervals_value = min_volume.data if smart_intervals_value.nil?
          smart_intervals_value = smart_intervals_value.ceil(volume_decimals.data)

          return Result::Success.new([smart_intervals_value, min_volume.data].max) if force_smart_intervals

          Result::Success.new([min_volume.data, volume].max)
        end

        def common_order_params(symbol, _limit_only = false)
          {
            product_id: symbol
          }
        end

        def parse_request(request)
          response = JSON.parse(request.body)
          Rails.logger.info "Coinbase parse_request #{response.to_json}"
          if order_done?(request, response)
            order_id = response.fetch('order_id')
            Result::Success.new(offer_id: order_id)
          else
            error_to_failure([response['message']])
          end
        rescue JSON::ParserError
          Result::Failure.new('Could not parse Coinbase response', **RECOVERABLE)
        end

        def order_done?(request, response)
          success?(request) && response['success']
        end

        def success?(request)
          request.status == 200 && request.reason_phrase == 'OK'
        end

        def market_order?(parsed_response)
          parsed_response.dig('order', 'order_type') == 'MARKET'
        end

        def filled?(parsed_response)
          parsed_response.dig('order', 'completion_percentage') == '100'
        end
      end
    end
  end
end
