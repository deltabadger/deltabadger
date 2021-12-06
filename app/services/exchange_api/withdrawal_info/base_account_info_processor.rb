module ExchangeApi
  module WithdrawalInfo
    class BaseAccountInfoProcessor
      RECOVERABLE = { data: { recoverable: true }.freeze }.freeze

      def withdrawal_minimum(_currency)
        raise NotImplementedError
      end

      def withdrawal_fee(_currency)
        raise NotImplementedError
      end

      def withdrawal_currencies
        raise NotImplementedError
      end

      def available_wallets
        raise NotImplementedError
      end

      def available_funds(_currency)
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
