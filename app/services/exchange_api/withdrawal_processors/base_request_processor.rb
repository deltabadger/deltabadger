module ExchangeApi
  module WithdrawalProcessors
    class BaseRequestProcessor
      RECOVERABLE = { data: { recoverable: true }.freeze }.freeze

      def make_withdrawal(_params)
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
