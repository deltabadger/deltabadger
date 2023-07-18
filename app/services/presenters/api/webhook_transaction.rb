module Presenters
  module Api
    class WebhookTransaction < BaseService
      def call(transaction)
        {
          id: transaction.id,
          rate: transaction.rate,
          amount: transaction.amount,
          price: price(transaction.rate, transaction.amount),
          created_at: transaction.created_at.strftime('%F %I:%M %p'),
          created_at_timestamp: transaction.created_at.to_i,
          called_bot_type: transaction.called_bot_type
        }
      end

      private

      def price(rate, amount)
        return nil unless rate && amount

        rate * amount
      end
    end
  end
end
