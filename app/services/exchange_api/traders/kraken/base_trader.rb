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
          @client = get_client(api_key, api_secret)
          @market = market
          @map_errors = map_errors
          @options = options
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

        def common_order_params(symbol)
          {
            pair: symbol,
            trading_agreement: ('agree' if @options[:german_trading_agreement])
          }.compact
        end

        def smart_volume(symbol, price, rate, force_smart_intervals)
          volume_decimals = @market.base_decimals(symbol)
          return volume_decimals unless volume_decimals.success?

          volume = (price / rate).ceil(volume_decimals.data)
          min_volume = @market.minimum_order_volume(symbol)
          return min_volume unless min_volume.success?

          return Result::Success.new(min_volume.data) if force_smart_intervals

          Result::Success.new([min_volume.data, volume].max)
        end

        def parse_response(response)
          raise NotImplementedError
        end

        def orders
          raise NotImplementedError
        end
      end
    end
  end
end
