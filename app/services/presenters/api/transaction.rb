module Presenters
  module Api
    class Transaction < BaseService
      def call(transaction)
        {
          id: transaction.id,
          rate: transaction.rate,
          amount: transaction.amount,
          price: transaction.rate * transaction.amount,
          created_at: transaction.created_at.strftime('%D')
        }
      end
    end
  end
end
