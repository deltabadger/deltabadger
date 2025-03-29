module Presenters
  module Api
    class Log < BaseService
      PRICE_RANGE_VIOLATION_MESSAGE = 'Checking… The trigger price has not been met yet.'.freeze
      WITHDRAWAL_TO_SMALL_AMOUNT_MESSAGE = 'Checking… The balance has not reached the withdrawal size yet.'.freeze

      def call(transaction)
        {
          id: transaction.id,
          rate: transaction.rate,
          amount: transaction.amount,
          price: price(transaction.rate, transaction.amount),
          created_at: transaction.created_at.strftime('%F %I:%M:%S %p'),
          status: transaction.status,
          external_id: transaction.external_id,
          errors: errors(transaction)
        }
      end

      private

      def price(rate, amount)
        return nil if !(rate && amount)

        rate * amount
      end

      def errors(transaction)
        return [get_skipped_message(transaction)] if transaction.skipped?

        transaction.error_messages
      end

      def get_skipped_message(transaction)
        transaction.transaction_type == 'WITHDRAWAL' ? WITHDRAWAL_TO_SMALL_AMOUNT_MESSAGE : PRICE_RANGE_VIOLATION_MESSAGE
      end
    end
  end
end
