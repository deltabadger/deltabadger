module Presenters
  module Api
    class Bot < BaseService
      def initialize(
        next_bot_transaction_at: NextBotTransactionAt.new,
        transactions_repository: TransactionsRepository.new,
        next_result_fetching_at: NextResultFetchingAt.new
      )
        @next_bot_transaction_at = next_bot_transaction_at
        @transactions_repository = transactions_repository
        @next_result_fetching_at = next_result_fetching_at
      end

      def call(bot)
        transactions = @transactions_repository.successful_for_bot(bot)
        logs = @transactions_repository.for_bot(bot, limit: 10)

        {
          id: bot.id,
          settings: bot.settings,
          exchangeName: bot.exchange.name,
          exchangeId: bot.exchange.id,
          status: bot.status,
          transactions: transactions.first(10).map(&method(:present_transaction)),
          logs: logs.map(&method(:present_log)),
          stats: present_stats(bot, transactions),
          nowTimestamp: Time.now.to_i,
          nextResultFetchingTimestamp: next_result_fetching_timestamp(bot),
          nextTransactionTimestamp: next_transaction_timestamp(bot)
        }
      end

      private

      def next_result_fetching_timestamp(bot)
        @next_result_fetching_at.call(bot).to_i
      rescue StandardError
        nil
      end

      def next_transaction_timestamp(bot)
        @next_bot_transaction_at.call(bot).to_i
      rescue StandardError
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
