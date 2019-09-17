module Presenters
  module Api
    class Bot < BaseService
      def call(bot)
        {
          id: bot.id,
          settings: bot.settings,
          exchangeName: bot.exchange.name,
          status: bot.status,
          transactions: bot.transactions.last(10).map(&method(:present_transaction)),
          logs: bot.transactions.map(&method(:present_transaction_as_log))
        }
      end

      private

      def present_transaction(transaction)
        Presenters::Api::Transaction.call(transaction)
      end

      def present_transaction_as_log(transaction)
        Presenters::Api::Log.call(transaction)
      end
    end
  end
end
