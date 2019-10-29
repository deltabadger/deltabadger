module Presenters
  module Api
    class Stats < BaseService
      def call(bot:, transactions:)
        transactions_price_sum = transactions.sum(&:price)
        transactions_amount_sum = transactions.sum(&:amount)
        avarage_price = transactions_price_sum / transactions.length

        {
          bought: "#{transactions_amount_sum} BTC",
          spent: "$#{transactions_price_sum}".slice(0, 8),
          avaragePrice: "$#{avarage_price}".slice(0, 8),
          currentValue: bot.transactions.last.rate,
          profitLoss: {
            positive: [true, false].sample,
            value: '+$273.70 (+17%)'
          }
        }
      end
    end
  end
end
