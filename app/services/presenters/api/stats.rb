module Presenters
  module Api
    class Stats < BaseService
      def call(bot:, transactions:)
        return {} if transactions.empty?

        transactions_price_sum = transactions.sum(&:price)
        transactions_amount_sum = transactions.sum(&:amount)
        avarage_price = transactions_price_sum / transactions.length
        total_invested = transactions.sum('rate * amount')
        current_value = 12
        total_portfolio_value = transactions.sum('amount') * current_value
        profit_loss = total_portfolio_value - total_invested

        {
          bought: "#{transactions_amount_sum} BTC",
          spent: "$#{transactions_price_sum}".slice(0, 8),
          avaragePrice: "$#{avarage_price}".slice(0, 8),
          currentValue: bot.transactions.last.rate,
          profitLoss: {
            positive: profit_loss.positive?,
            value: "#{profit_loss.positive? ? '+' : '-'}#{profit_loss}".slice(0, 8)
          }
        }
      end
    end
  end
end
