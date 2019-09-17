module Presenters
  module Api
    class Log < BaseService
      def call(transaction)
        {
          id: transaction.id,
          rate: transaction.rate,
          amount: transaction.amount,
          price: price(transaction.rate, transaction.amount),
          created_at: transaction.created_at.strftime('%D'),
          status: transaction.status,
          offer_id: transaction.offer_id,
          errors: transaction.error_messages
        }
      end

      private

      def price(rate, amount)
        return nil if !(rate && amount)

        rate * amount
      end
    end
  end
end
