module Presenters
  module Api
    class Bot < BaseService
      def initialize(parse_interval: ParseInterval.new)
        @parse_interval = parse_interval
      end

      def call(bot)
        {
          id: bot.id,
          settings: bot.settings,
          exchangeName: bot.exchange.name,
          status: bot.status,
          transactions: transactions(bot.transactions),
          logs: logs(bot.transactions),
          stats: present_stats(bot),
          nextTransactionTimestamp: next_transaction_timestamp(bot),
          charts: {
            PortfolioValueOverTime: Charts::PortfolioValueOverTime.call(bot)
          }
        }
      end

      private

      def next_transaction_timestamp(bot)
        return nil if !bot.transactions.exists?

        interval = @parse_interval.call(bot.settings)
        (bot.transactions.last.created_at + interval).to_i
      end

      def transactions(transactions)
        transactions.last(10).map(&method(:present_transaction))
      end

      def logs(logs)
        logs.last(10).map(&method(:present_transaction_as_log))
      end

      def present_stats(bot)
        Presenters::Api::Stats.call(bot: bot, transactions: bot.transactions)
      end

      def present_transaction(transaction)
        Presenters::Api::Transaction.call(transaction)
      end

      def present_transaction_as_log(transaction)
        Presenters::Api::Log.call(transaction)
      end
    end
  end
end
