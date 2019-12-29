module Presenters
  module Api
    class Bot < BaseService
      def initialize(
        parse_interval: ParseInterval.new,
        transactions_repository: TransactionsRepository.new
      )

        @parse_interval = parse_interval
        @transactions_repository = transactions_repository
      end

      def call(bot)
        transactions = @transactions_repository.successful_for_bot(bot)
        logs = @transactions_repository.for_bot(bot, limit: 10)

        {
          id: bot.id,
          settings: bot.settings,
          exchangeName: bot.exchange.name,
          status: bot.status,
          transactions: transactions.first(10).map(&method(:present_transaction)),
          logs: logs.map(&method(:present_log)),
          stats: present_stats(bot, transactions),
          nowTimestamp: Time.now.to_i,
          nextTransactionTimestamp: next_transaction_timestamp(bot)
        }
      end

      private

      def next_transaction_timestamp(bot)
        return nil if !bot.transactions.exists?

        bot.reload
        interval = @parse_interval.call(bot)
        interval.since(bot.last_transaction.created_at).to_i
      rescue
        nil
      end

      def logs(bot)
        @transactions_repository
          .for_bot(bot, limit: 10)
          .map(&method(:present_transaction_as_log))
      end

      def present_stats(bot, transactions)
        Presenters::Api::Stats.call(bot: bot, transactions: transactions)
      end

      def present_transaction(transaction)
        Presenters::Api::Transaction.call(transaction)
      end

      def present_log(transaction)
        Presenters::Api::Log.call(transaction)
      end
    end
  end
end
