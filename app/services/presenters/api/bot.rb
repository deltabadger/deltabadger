module Presenters
  module Api
    class Bot < BaseService
      def call(bot)
        {
          id: bot.id,
          settings: bot.settings,
          exchangeName: bot.exchange.name,
          status: bot.status,
          transactions: bot.transactions.map(&method(:present_transaction))
        }
      end

      private

      def present_transaction(transaction)
        Presenters::Api::Transaction.call(transaction)
      end
    end
  end
end
