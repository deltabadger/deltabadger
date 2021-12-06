module Presenters
  module Api
    class WithdrawalTransaction < BaseService
      def call(transaction)
        {
          id: transaction.id,
          amount: transaction.amount,
          created_at: transaction.created_at.strftime('%F %I:%M %p'),
          created_at_timestamp: transaction.created_at.to_i
        }
      end
    end
  end
end
