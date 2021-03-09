module ExchangeApi
  module Traders
    class BaseTrader
      RECOVERABLE = { data: { recoverable: true }.freeze }.freeze
      NOT_FETCHED = { data: { fetched: false }.freeze }.freeze

      def buy
        raise NotImplementedError
      end

      def sell
        raise NotImplementedError
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
