require 'result'

module ExchangeApi
  module Clients
    class Base
      RECOVERABLE = { data: { recoverable: true }.freeze }.freeze

      def current_bid_ask_price(_settings)
        raise NotImplementedError
      end

      def validate_credentials
        raise NotImplementedError
      end

      def buy
        raise NotImplementedError
      end

      def sell
        raise NotImplementedError
      end

      def current_price(settings)
        result = current_bid_ask_price(settings)
        return result unless result.success?

        price = result.data
        Result::Success.new((price.bid + price.ask) / 2)
      end

      def current_bid_price(settings)
        result = current_bid_ask_price(settings)
        return result unless result.success?

        Result::Success.new(result.data.bid)
      end

      def current_ask_price(settings)
        result = current_bid_ask_price(settings)
        return result unless result.success?

        Result::Success.new(result.data.ask)
      end

      protected

      def error_to_failure(error)
        mapped_error = @map_errors.call(error)
        Result::Failure.new(
          *mapped_error.message, data: { recoverable: mapped_error.recoverable }
        )
      end
    end
  end
end
