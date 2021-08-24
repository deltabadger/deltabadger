module Presenters
  module Api
    class Log < BaseService
      PRICE_RANGE_VIOLATION_MESSAGE = 'Trigger price not met'.freeze

      def call(transaction)
        {
          id: transaction.id,
          rate: transaction.rate,
          amount: transaction.amount,
          price: price(transaction.rate, transaction.amount),
          created_at: transaction.created_at.strftime('%F %I:%M:%S %p'),
          status: transaction.status,
          offer_id: transaction.offer_id,
          errors: errors(transaction)
        }
      end

      private

      def price(rate, amount)
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
