module Presenters
  module Api
    class TradingTransaction < BaseService
      def call(transaction)
        {
          id: transaction.id,
          price: transaction.price,
          amount: transaction.amount,
          quote_amount: transaction.quote_amount || quote_amount(transaction.price, transaction.amount),
          created_at: transaction.created_at.in_time_zone(transaction.bot.user.time_zone).strftime('%F %I:%M %p'),
          created_at_timestamp: transaction.created_at.to_i
        }
      end

      private

      def quote_amount(rate, amount)
        return nil unless rate && amount

        rate * amount
      end
    end
  end
end
