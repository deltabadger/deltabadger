module Presenters
  module Api
    class Log < BaseService
      PRICE_RANGE_VIOLATION_MESSAGE = 'Checking… The trigger price has not been met yet.'.freeze

      def call(transaction)
        {
          id: transaction.id,
          price: transaction.price,
          amount: transaction.amount,
          quote_amount: transaction.amount || quote_amount(transaction.price, transaction.amount),
          created_at: transaction.created_at.strftime('%F %I:%M:%S %p'),
          status: transaction.status,
          external_id: transaction.external_id,
          errors: errors(transaction)
        }
      end

      private

      def quote_amount(rate, amount)
        return nil if !(rate && amount)

        rate * amount
      end

      def errors(transaction)
        return [PRICE_RANGE_VIOLATION_MESSAGE] if transaction.skipped?

        transaction.error_messages
      end
    end
  end
end
