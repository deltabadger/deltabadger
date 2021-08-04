require 'result'

module ExchangeApi
  module Traders
    module Kraken
      class BaseTrader < ExchangeApi::Traders::BaseTrader
        include ExchangeApi::Clients::Kraken

        def initialize(
          api_key:,
          api_secret:,
          market: ExchangeApi::Markets::Kraken::Market.new,
          map_errors: ExchangeApi::MapErrors::Kraken.new,
          options: {}
        )
          @client = get_base_client(api_key, api_secret)
          @market = market
          @map_errors = map_errors
          @options = options
        end

        def fetch_order_by_id(order_id)
          order_data = orders.fetch(order_id, nil)

          if order_data.nil?
            res = @client.send :post_private, 'QueryOrders', txid: order_id
            order_data = res.dig('result').fetch(order_id, nil)
          end

          return error_to_failure([order_data.fetch('reason')]) if canceled?(order_data)
          return Result::Failure.new('Waiting for Kraken response', NOT_FETCHED) if opened?(order_data)

          rate = placed_order_rate(order_data)
          amount = order_data.fetch('vol').to_f

          Result::Success.new(
            offer_id: order_id,
            amount: amount,
            rate: rate
          )
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not make Kraken order', RECOVERABLE)
        end

        private

        def place_order(order_params)
          response = @client.add_order(order_params)
          return error_to_failure(response.fetch('error')) if response.fetch('error').any?

          result = parse_response(response)
          Result::Success.new(result)
        rescue StandardError => e
          Raven.capture_exception(e)
          Result::Failure.new('Could not make Kraken order', RECOVERABLE)
        end

        def parse_response(response)
          created_order = response.fetch('result')
          offer_id = created_order.fetch('txid').first
          { offer_id: offer_id }
        end

        def common_order_params(symbol)
          {
            pair: symbol,
            trading_agreement: ('agree' if @options[:german_trading_agreement])
          }.compact
        end

        def smart_volume(symbol, price, rate, force_smart_intervals, smart_intervals_value)
          volume_decimals = @market.base_decimals(symbol)
          return volume_decimals unless volume_decimals.success?

          volume = (price / rate).floor(volume_decimals.data)
          min_volume = @market.minimum_order_volume(symbol)
          return min_volume unless min_volume.success?

          smart_intervals_value = min_volume.data if smart_intervals_value.nil?
          smart_intervals_value = smart_intervals_value.floor(volume_decimals.data)

          return Result::Success.new([smart_intervals_value, min_volume.data].max) if force_smart_intervals

          Result::Success.new([min_volume.data, volume].max)
        end

        def placed_order_rate(order_data)
          order_data.fetch('price').to_f
        end

        def orders
          raise NotImplementedError
        end

        def opened?(order_data)
          order_data.nil? || order_data.fetch('status') != 'closed'
        end

        def canceled?(order_data)
          !order_data.nil? && order_data.fetch('status') == 'canceled'
        end
      end
    end
  end
end
