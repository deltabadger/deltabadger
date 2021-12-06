module Presenters
  module Api
    class TradingTransaction < BaseService
      def call(transaction)
        {
          id: transaction.id,
          rate: transaction.rate,
          amount: transaction.amount,
          price: price(transaction.rate, transaction.amount),
          created_at: transaction.created_at.strftime('%F %I:%M %p'),
          created_at_timestamp: transaction.created_at.to_i
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
